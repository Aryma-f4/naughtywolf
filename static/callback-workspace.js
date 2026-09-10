(() => {
  const terminalStates = new Set(["completed", "error", "cancelled"]);

  function createWorkspace(root, dependencies = {}) {
    const doc = dependencies.document || document;
    const request = dependencies.fetch || fetch;
    const EventStream = dependencies.EventSource || EventSource;
    const tasks = new Map();
    const cards = new Map();
    const list = root.querySelector("[data-task-list]");
    const empty = root.querySelector("[data-task-empty]");
    const search = root.querySelector("[data-task-search]");
    const stateFilter = root.querySelector("[data-task-state-filter]");
    const errorFilter = root.querySelector("[data-task-error-filter]");
    const loadOlder = root.querySelector("[data-load-older]");
    const connectionState = root.querySelector("[data-connection-state]");
    const offlineQueue = root.querySelector("[data-offline-queue]");
    const commandInput = root.querySelector("[data-command-input]");
    const commandArguments = root.querySelector("[data-command-arguments]");
    const form = root.querySelector("[data-task-form]");
    let nextBefore = null;
    let eventSource = null;
    let reconnecting = false;
    let historyIndex = -1;
    let draft = "";
    let closed = false;

    const element = (tag, className, text) => {
      const node = doc.createElement(tag);
      if (className) node.className = className;
      if (text !== undefined && text !== null) node.textContent = String(text);
      return node;
    };

    const field = (label, value, marker) => {
      const row = element("div", "task-field");
      row.append(element("dt", "task-field-label", label));
      const output = element("dd", "task-field-value", value ?? "—");
      if (marker) output.dataset[marker] = "";
      row.append(output);
      return row;
    };

    const createCard = task => {
      const card = element("article", "callback-task-card");
      card.dataset.taskCard = "";
      card.dataset.taskId = task.id;
      const header = element("header", "callback-task-header");
      const identity = element("div", "callback-task-identity");
      identity.append(element("code", "callback-task-command"));
      identity.append(element("span", "callback-task-id"));
      const state = element("span", "status-pill");
      state.dataset.taskState = "";
      header.append(identity, state);
      const meta = element("dl", "callback-task-meta");
      meta.append(
        field("Operator", "", "taskOperator"),
        field("Submitted", "", "taskCreated"),
        field("Updated", "", "taskUpdated"),
        field("Processing", "", "taskProcessing"),
        field("Completed", "", "taskCompleted"),
      );
      const argumentsBlock = element("details", "callback-task-arguments");
      argumentsBlock.append(element("summary", "", "Arguments"));
      const argumentsValue = element("pre");
      argumentsValue.dataset.taskArguments = "";
      argumentsBlock.append(argumentsValue);
      const output = element("div", "callback-task-output");
      const stdout = element("pre", "task-stdout");
      stdout.dataset.taskStdout = "";
      const stderr = element("pre", "task-stderr");
      stderr.dataset.taskStderr = "";
      const exitCode = element("span", "task-exit-code");
      exitCode.dataset.taskExitCode = "";
      output.append(element("strong", "", "stdout"), stdout, element("strong", "", "stderr"), stderr, exitCode);
      const actions = element("footer", "callback-task-actions");
      const retry = element("button", "btn btn-ghost", "Retry");
      retry.type = "button";
      retry.dataset.taskAction = "retry";
      const cancel = element("button", "btn btn-ghost", "Cancel");
      cancel.type = "button";
      cancel.dataset.taskAction = "cancel";
      actions.append(retry, cancel);
      card.append(header, meta, argumentsBlock, output, actions);
      return card;
    };

    const updateCard = (card, task) => {
      card.dataset.taskStatus = task.status;
      card.dataset.taskCommand = task.command;
      card.dataset.taskOperator = task.operator_name || task.operator_id || "system";
      card.dataset.taskError = task.status === "error" || Boolean(task.stderr) ? "true" : "false";
      card.querySelector(".callback-task-command").textContent = task.command;
      card.querySelector(".callback-task-id").textContent = task.id;
      const state = card.querySelector("[data-task-state]");
      state.className = `status-pill status-${task.state_class || "neutral"}`;
      state.textContent = task.state_label || task.status;
      card.querySelector("[data-task-operator]").textContent = task.operator_name || task.operator_id || "System";
      card.querySelector("[data-task-created]").textContent = task.created_at || "—";
      card.querySelector("[data-task-updated]").textContent = task.updated_at || "—";
      card.querySelector("[data-task-processing]").textContent = task.processing_at || "—";
      card.querySelector("[data-task-completed]").textContent = task.completed_at || "—";
      card.querySelector("[data-task-arguments]").textContent = JSON.stringify(task.arguments ?? [], null, 2);
      card.querySelector("[data-task-stdout]").textContent = task.stdout || "";
      card.querySelector("[data-task-stderr]").textContent = task.stderr || "";
      card.querySelector("[data-task-exit-code]").textContent = task.exit_code === null || task.exit_code === undefined ? "Exit code —" : `Exit code ${task.exit_code}`;
      const actions = Array.from(card.querySelectorAll("[data-task-action]"));
      const retry = actions.find(button => button.dataset.taskAction === "retry") || null;
      const cancel = actions.find(button => button.dataset.taskAction === "cancel") || null;
      if (retry) retry.hidden = !terminalStates.has(task.status);
      if (cancel) cancel.hidden = terminalStates.has(task.status) || Boolean(task.cancellation_requested_at);
      let waiting = card.querySelector("[data-task-waiting]");
      if (root.dataset.online === "false" && task.status === "pending") {
        if (!waiting) {
          waiting = element("p", "offline-task-note", "Waiting for callback check-in");
          waiting.dataset.taskWaiting = "";
          card.append(waiting);
        }
      } else if (waiting) {
        waiting.remove();
      }
    };

    const orderedTasks = () => [...tasks.values()].sort((a, b) =>
      b.created_at.localeCompare(a.created_at) || b.id.localeCompare(a.id));

    const applyFilters = () => {
      const query = (search?.value || "").trim().toLowerCase();
      const state = stateFilter?.value || "";
      const errors = errorFilter?.value || "";
      for (const task of tasks.values()) {
        const card = cards.get(task.id);
        const haystack = [task.command, task.operator_name, task.operator_id, task.stdout, task.stderr, JSON.stringify(task.arguments ?? [])]
          .filter(Boolean).join(" ").toLowerCase();
        const matches = (!query || haystack.includes(query))
          && (!state || task.status === state)
          && (!errors || errors !== "errors" || task.status === "error" || Boolean(task.stderr));
        card.hidden = !matches;
      }
      if (empty) empty.hidden = tasks.size !== 0;
    };

    const renderOrder = () => {
      list.replaceChildren(...orderedTasks().map(task => cards.get(task.id)));
      applyFilters();
    };

    const upsert = task => {
      if (!task || !task.id) return;
      tasks.set(task.id, { ...(tasks.get(task.id) || {}), ...task });
      let card = cards.get(task.id);
      if (!card) {
        card = createCard(task);
        cards.set(task.id, card);
      }
      updateCard(card, tasks.get(task.id));
      renderOrder();
    };

    const loadPage = async (before = null) => {
      const suffix = before ? `?limit=20&before=${encodeURIComponent(before)}` : "?limit=20";
      const response = await request(`${root.dataset.tasksEndpoint}${suffix}`, { credentials: "same-origin" });
      if (!response.ok) throw new Error(`Task history request failed (${response.status})`);
      const page = await response.json();
      for (const task of page.tasks || []) upsert(task);
      nextBefore = page.next_before || null;
      if (loadOlder) {
        loadOlder.hidden = !nextBefore;
        loadOlder.disabled = false;
      }
      return page;
    };

    const setConnection = value => {
      if (connectionState) connectionState.textContent = value;
    };

    const connect = () => {
      if (closed || !root.dataset.eventsEndpoint || !EventStream) return;
      eventSource = new EventStream(root.dataset.eventsEndpoint);
      eventSource.addEventListener("task", event => {
        try { upsert(JSON.parse(event.data)); } catch (_) { /* ignore malformed snapshots */ }
      });
      eventSource.onopen = () => {
        setConnection("Connected");
        if (reconnecting) {
          reconnecting = false;
          loadPage().catch(() => setConnection("Reconnecting"));
        }
      };
      eventSource.onerror = () => {
        reconnecting = true;
        setConnection("Reconnecting");
      };
    };

    search?.addEventListener("input", applyFilters);
    stateFilter?.addEventListener("change", applyFilters);
    errorFilter?.addEventListener("change", applyFilters);
    loadOlder?.addEventListener("click", async () => {
      if (!nextBefore) return;
      loadOlder.disabled = true;
      try { await loadPage(nextBefore); } finally { loadOlder.disabled = false; }
    });

    commandInput?.addEventListener("keydown", event => {
      const history = orderedTasks().map(task => task.command).filter(Boolean);
      if (event.key === "ArrowUp" && history.length) {
        event.preventDefault();
        if (historyIndex < 0) draft = commandInput.value;
        historyIndex = Math.min(historyIndex + 1, history.length - 1);
        commandInput.value = history[historyIndex];
      } else if (event.key === "ArrowDown" && historyIndex >= 0) {
        event.preventDefault();
        historyIndex -= 1;
        commandInput.value = historyIndex >= 0 ? history[historyIndex] : draft;
      }
    });

    form?.addEventListener("submit", async event => {
      event.preventDefault();
      const command = commandInput.value.trim();
      if (!command) return;
      const argumentsList = commandArguments.value.trim() ? commandArguments.value.trim().split(/\s+/) : [];
      try {
        const response = await request(root.dataset.tasksEndpoint, {
          method: "POST",
          credentials: "same-origin",
          headers: { "content-type": "application/json", "x-csrf-token": root.dataset.csrfToken },
          body: JSON.stringify({ command, arguments: argumentsList, timeout_ms: 30000 }),
        });
        if (!response.ok) throw new Error(`Task submission failed (${response.status})`);
        upsert(await response.json());
        commandInput.value = "";
        commandArguments.value = "";
        historyIndex = -1;
      } catch (_) {
        setConnection("Unable to submit task — check connection and retry.");
      }
    });

    list?.addEventListener("click", async event => {
      const button = event.target.closest?.("[data-task-action]");
      const card = button?.closest?.("[data-task-card]");
      if (!button || !card) return;
      const action = button.dataset.taskAction;
      try {
        const response = await request(`${root.dataset.tasksEndpoint}/${encodeURIComponent(card.dataset.taskId)}/${action}`, {
          method: "POST",
          credentials: "same-origin",
          headers: { "x-csrf-token": root.dataset.csrfToken },
        });
        if (!response.ok) throw new Error(`Task action failed (${response.status})`);
        upsert(await response.json());
      } catch (_) {
        setConnection(`Unable to ${action} task — check connection and retry.`);
      }
    });

    if (offlineQueue) {
      offlineQueue.hidden = root.dataset.online !== "false";
      offlineQueue.textContent = "Callback is offline. New tasks stay queued until its next check-in.";
    }
    setConnection("Connecting");
    const ready = loadPage()
      .then(() => connect())
      .catch(() => setConnection("History unavailable — retry"));

    return {
      ready,
      upsert,
      close() {
        if (closed) return;
        closed = true;
        eventSource?.close();
      },
    };
  }

  const api = {
    init(root = document) {
      const workspace = root.matches?.("[data-callback-tasking]")
        ? root
        : root.querySelector?.("[data-callback-tasking]");
      if (!workspace || workspace.dataset.taskingBound === "true") return null;
      workspace.dataset.taskingBound = "true";
      return createWorkspace(workspace);
    },
  };

  if (typeof module === "object" && module.exports) module.exports = { createWorkspace };
  if (typeof window !== "undefined") window.NWCallbackWorkspace = api;
})();
