// An interactive terminal for a callback's attached shell.
//
// The shell itself lives on the implant; this is the browser end of a pipe. It
// does three things the rest of the workspace cannot:
//
//   * renders a real terminal stream, including ANSI colour, so `ls --color`
//     and full-screen programs look the way they do in a terminal;
//   * turns key presses into the bytes a terminal sends (CR for Enter, DEL for
//     Backspace, escape sequences for arrows), instead of a plain string;
//   * shows output the instant the callback writes it, and replays what was
//     already written when the page is opened.
//
// No dependencies: the escape-sequence handling is the small part that matters,
// and the rest is text nodes.

(() => {
  const CSI = "[";

  // The subset of SGR that changes how text looks. Everything else in an escape
  // sequence is passed through as text, so a sequence we do not model shows up
  // as noise rather than silently swallowing output.
  const COLOURS = [
    "#1b1f27", "#f47067", "#5cc98a", "#e3c26a", "#6aa9f0", "#c792ea", "#4fd6c8", "#c8d3e0",
    "#5a6472", "#ff8b82", "#7fe0a6", "#f2d68a", "#8cc0ff", "#dcb0ff", "#6ee7dc", "#eef3fa",
  ];

  const escapeHtml = value =>
    String(value).replace(/[&<>"']/g, character => ({
      "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
    })[character]);

  /**
   * Turn terminal output into HTML.
   *
   * Only the sequences that affect colour are interpreted; everything else is
   * escaped text. Carriage returns are honoured (a progress line rewrites
   * itself) and a bare backspace erases the character before it, which is what
   * a line editor expects.
   */
  const render = (input, style = {}) => {
    let html = "";
    let open = 0;
    let text = "";

    const flush = () => {
      if (!text) return;
      html += escapeHtml(text);
      text = "";
    };
    const closeAll = () => {
      while (open > 0) {
        html += "</span>";
        open -= 1;
      }
    };

    for (let index = 0; index < input.length; index += 1) {
      const character = input[index];

      if (input.startsWith(CSI, index)) {
        const end = input.indexOf("m", index);
        if (end === -1) {
          text += character;
          continue;
        }
        const body = input.slice(index + CSI.length, end);
        if (!/^[0-9;]*$/.test(body)) {
          text += input.slice(index, end + 1);
          index = end;
          continue;
        }
        flush();
        closeAll();
        for (const code of (body || "0").split(";")) {
          const value = Number(code || 0);
          if (value === 0) continue;
          if (value === 1) html += `<span style="${style.weight || "font-weight:600"}">`;
          else if (value >= 30 && value <= 37) html += `<span style="color:${COLOURS[value - 30]}">`;
          else if (value >= 90 && value <= 97) html += `<span style="color:${COLOURS[value - 90 + 8]}">`;
          else if (value >= 40 && value <= 47) html += `<span style="background:${COLOURS[value - 40]}">`;
          else if (value === 39) html += `<span style="color:inherit">`;
          else if (value === 49) html += `<span style="background:transparent">`;
          open += 1;
        }
        index = end;
        continue;
      }

      if (character === "\r") {
        // A carriage return rewrites the current line rather than adding one.
        flush();
        html += "<span data-carriage-return></span>";
        continue;
      }
      if (character === "\b") {
        flush();
        html += "<span data-backspace></span>";
        continue;
      }
      text += character;
    }
    flush();
    closeAll();
    return html;
  };

  /**
   * The bytes a key press means to a terminal.
   *
   * Anything not listed returns null: the key is not ours to send, and sending
   * the raw character anyway would produce output the operator never typed.
   */
  const keyToBytes = event => {
    switch (event.key) {
      case "Enter": return "\r";
      case "Backspace": return "\x7f";
      case "Tab": return "\t";
      case "Escape": return "\x1b";
      case "ArrowUp": return "\x1b[A";
      case "ArrowDown": return "\x1b[B";
      case "ArrowRight": return "\x1b[C";
      case "ArrowLeft": return "\x1b[D";
      case "Home": return "\x1b[H";
      case "End": return "\x1b[F";
      case "PageUp": return "\x1b[5~";
      case "PageDown": return "\x1b[6~";
      case "Delete": return "\x1b[3~";
      default: break;
    }
    if (event.ctrlKey && event.key.length === 1) {
      const code = event.key.toUpperCase().charCodeAt(0);
      // Ctrl-A..Ctrl-Z map onto 1..26; anything else is left alone.
      if (code >= 65 && code <= 90) return String.fromCharCode(code - 64);
      if (event.key === "[") return "\x1b";
      if (event.key === "]") return "\x1d";
    }
    if (event.metaKey || event.altKey) return null;
    return event.key.length === 1 ? event.key : null;
  };

  /** Bytes carried as base64, decoded into text a terminal can render. */
  const decodeChunk = encoded => {
    try {
      const binary = atob(encoded);
      const bytes = new Uint8Array(binary.length);
      for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
      return new TextDecoder("utf-8", { fatal: false }).decode(bytes);
    } catch (_) {
      return "";
    }
  };

  function createTerminal(root, dependencies = {}) {
    const request = dependencies.fetch || (typeof fetch !== 'undefined' ? fetch : null);
    const EventStream = dependencies.EventSource || (typeof EventSource !== 'undefined' ? EventSource : null);
    const screen = root.querySelector("[data-shell-screen]");
    const placeholder = root.querySelector("[data-shell-placeholder]");
    const state = root.querySelector("[data-shell-state]");
    const message = root.querySelector("[data-shell-message]");
    const attach = root.querySelector("[data-shell-attach]");
    const program = root.querySelector("[data-shell-program]");
    const form = root.querySelector("[data-shell-form]");
    const input = root.querySelector("[data-shell-input]");
    const send = root.querySelector("[data-shell-send]");
    const close = root.querySelector("[data-shell-close]");
    const bar = root.querySelector("[data-shell-session-bar]");
    const badge = root.querySelector("[data-shell-badge]");
    const detail = root.querySelector("[data-shell-session-detail]");
    const endpoint = root.dataset.shellsEndpoint;
    const streamEndpoint = root.dataset.shellStreamEndpoint;

    let shells = new Map();
    let activeId = null;
    let stream = null;
    let closed = false;
    let busy = false;

    const setState = value => {
      if (!state) return;
      state.textContent = value;
      // Drives the dot beside the state label.
      state.dataset.state = value === "Attached" ? "attached" : "detached";
    };
    const say = value => { if (message) message.textContent = value; };

    const write = text => {
      if (!screen || !text) return;
      if (placeholder) placeholder.hidden = true;
      const atBottom = screen.scrollTop + screen.clientHeight >= screen.scrollHeight - 24;
      screen.insertAdjacentHTML("beforeend", render(text));
      // Keep the buffer bounded: a long-running shell would otherwise grow the
      // page without limit.
      while (screen.childElementCount > 400) screen.removeChild(screen.firstElementChild);
      if (atBottom) screen.scrollTop = screen.scrollHeight;
    };

    const describe = shell => {
      if (!shell) return "no shell";
      const kind = shell.pty ? "pty" : "pipes";
      const argv = [shell.program, ...(shell.args || [])].filter(Boolean).join(" ");
      return shell.state === "closed"
        ? `${argv || "shell"} closed${shell.exit_code === null || shell.exit_code === undefined ? "" : ` (exit ${shell.exit_code})`}`
        : `${argv || "shell"} · ${kind} · ${shell.state}`;
    };

    const renderBar = () => {
      if (!bar) return;
      const shell = shells.get(activeId);
      bar.hidden = !shell;
      if (!shell) return;
      if (badge) badge.textContent = shell.pty ? "PTY" : "PIPES";
      if (detail) detail.textContent = describe(shell);
      const running = shell.state !== "closed";
      if (input) input.disabled = !running;
      if (send) send.disabled = !running;
      if (attach) attach.disabled = false;
    };

    const selectShell = (id, { render_ = true } = {}) => {
      if (!shells.has(id)) return;
      activeId = id;
      if (render_) renderBar();
    };

    const sendInput = async data => {
      // Nothing typed goes anywhere until a shell is actually running: sending
      // to a closed shell would queue bytes the callback will never read.
      if (!activeId || busy || shells.get(activeId)?.state === "closed") return;
      busy = true;
      try {
        const response = await request(`${endpoint}/input`, {
          method: "POST",
          credentials: "same-origin",
          headers: { "content-type": "application/json", "x-csrf-token": root.dataset.csrfToken },
          body: JSON.stringify({ shell_id: activeId, data }),
        });
        if (!response.ok) throw new Error(`input failed (${response.status})`);
        const result = await response.json();
        say(
          result.delivery === "push"
            ? "Sent over the live channel."
            : "No live socket: the callback will read this on its next check-in.",
        );
      } catch (_) {
        say("Unable to send input — check the connection and retry.");
      } finally {
        busy = false;
      }
    };

    const openShell = async () => {
      if (!endpoint) return;
      attach && (attach.disabled = true);
      try {
        const response = await request(endpoint, {
          method: "POST",
          credentials: "same-origin",
          headers: { "content-type": "application/json", "x-csrf-token": root.dataset.csrfToken },
          body: JSON.stringify({
            program: program?.value || "",
            args: program?.value ? [] : [],
          }),
        });
        if (!response.ok) throw new Error(`attach failed (${response.status})`);
        const result = await response.json();
        shells.set(result.shell.id, { ...result.shell, scrollback: "" });
        selectShell(result.shell.id);
        setState(result.delivery === "push" ? "Attached" : "Waiting for check-in");
        say(
          result.delivery === "push"
            ? "Shell requested over the live channel; it is starting now."
            : "No live socket for this callback. The shell starts at its next check-in.",
        );
      } catch (_) {
        say("Unable to attach a shell to this callback.");
      } finally {
        attach && (attach.disabled = false);
      }
    };

    const closeShell = async () => {
      const id = activeId;
      if (!id) return;
      try {
        await request(`${endpoint}/close`, {
          method: "POST",
          credentials: "same-origin",
          headers: { "content-type": "application/json", "x-csrf-token": root.dataset.csrfToken },
          body: JSON.stringify({ shell_id: id }),
        });
      } catch (_) {
        say("Unable to close the shell; it will end when the callback disconnects.");
      }
    };

    const handle = message => {
      if (!message || typeof message !== "object") return;
      switch (message.type) {
        case "snapshot": {
          // Reattaching to a shell that is already running: replay its output so
          // the operator picks up the session rather than a blank screen.
          if (screen) screen.innerHTML = "";
          shells = new Map();
          for (const shell of message.shells || []) {
            shells.set(shell.id, shell);
            write(decodeChunk(shell.scrollback || ""));
          }
          const running = [...shells.values()].filter(shell => shell.state !== "closed");
          if (running.length) selectShell(running[running.length - 1].id);
          else if ((message.shells || []).length) selectShell(message.shells[message.shells.length - 1].id);
          setState(running.length ? "Attached" : "Detached");
          break;
        }
        case "opened": {
          const known = shells.get(message.shell.id);
          shells.set(message.shell.id, { ...message.shell, scrollback: known?.scrollback || "" });
          selectShell(message.shell.id);
          setState("Attached");
          say(`Shell running on ${message.shell.program || "the platform shell"}.`);
          break;
        }
        case "output": {
          if (activeId && message.shell_id !== activeId && !shells.has(message.shell_id)) break;
          write(decodeChunk(message.data || ""));
          break;
        }
        case "closed": {
          const shell = shells.get(message.shell_id);
          if (shell) shells.set(message.shell_id, { ...shell, state: "closed", exit_code: message.code });
          if (message.shell_id === activeId) {
            setState("Detached");
            say(
              message.code === null || message.code === undefined
                ? "Shell closed."
                : `Shell exited with status ${message.code}.`,
            );
          }
          renderBar();
          break;
        }
        case "failed": {
          say(`Shell failed: ${message.reason}`);
          setState("Detached");
          break;
        }
        default: break;
      }
    };

    const connect = () => {
      if (closed || !streamEndpoint || !EventStream) return;
      stream = new EventStream(streamEndpoint);
      stream.addEventListener("shell", event => {
        try { handle(JSON.parse(event.data)); } catch (_) { /* ignore malformed frames */ }
      });
      stream.onopen = () => setState(shells.size ? "Attached" : "Detached");
      stream.onerror = () => setState("Reconnecting");
    };

    // `await`-able handlers: the tests drive them directly.
    attach?.addEventListener("click", () => openShell());
    close?.addEventListener("click", closeShell);
    form?.addEventListener("submit", event => {
      event.preventDefault();
      const value = input?.value ?? "";
      if (input) input.value = "";
      // A submitted line is a command followed by the Enter a terminal sends.
      return value ? sendInput(`${value}\r`) : undefined;
    });
    input?.addEventListener("keydown", event => {
      if (event.ctrlKey && event.key === "v") return; // paste is the browser's
      const bytes = keyToBytes(event);
      if (bytes === null) return undefined;
      event.preventDefault();
      return sendInput(bytes);
    });
    screen?.addEventListener("click", () => input?.focus());

    if (root.dataset.shellCapable !== "true") {
      if (attach) attach.disabled = true;
      say("This callback does not advertise an interactive shell.");
    }

    return {
      handle,
      keyToBytes,
      sendInput,
      connect,
      close() {
        closed = true;
        stream?.close();
      },
    };
  }

  const api = {
    init(root = document) {
      const panel = root.matches?.("[data-shell-panel]") ? root : root.querySelector?.("[data-shell-panel]");
      if (!panel || panel.dataset.shellBound === "true") return null;
      panel.dataset.shellBound = "true";
      const terminal = createTerminal(panel);
      terminal.connect?.();
      return terminal;
    },
  };

  if (typeof module === "object" && module.exports) module.exports = { createTerminal, render, keyToBytes, decodeChunk };
  if (typeof window !== "undefined") {
    window.NWShell = api;
    window.NWShellConnect = () => api.init(document);
  }
})();