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
    const processPanel = root.querySelector("[data-process-panel]");
    const processDataset = processPanel?.dataset || root.dataset;
    const processSearch = root.querySelector("[data-process-search]");
    const processBody = root.querySelector("[data-process-table-body]");
    const processEmpty = root.querySelector("[data-process-empty]");
    const processSnapshotAge = root.querySelector("[data-process-snapshot-age]");
    const processMessage = root.querySelector("[data-process-message]");
    const processRefresh = root.querySelector("[data-process-refresh]");
    const processDetail = root.querySelector("[data-process-detail]");
    const processConfirm = root.querySelector("[data-process-confirm]");
    const processConfirmText = root.querySelector("[data-process-confirm-text]");
    const processConfirmSubmit = root.querySelector("[data-process-confirm-submit]");
    const processConfirmCancel = root.querySelector("[data-process-confirm-cancel]");
    const processTaskLink = root.querySelector("[data-process-task-link]");
    let processRows = [];
    let processSortKey = "pid";
    let processSortDirection = 1;
    let selectedProcess = null;
    const filePanel = root.querySelector("[data-file-panel]");
    const fileDataset = filePanel?.dataset || root.dataset;
    const filePathForm = root.querySelector("[data-file-path-form]");
    const filePathInput = root.querySelector("[data-file-path]");
    const fileBreadcrumbs = root.querySelector("[data-file-breadcrumbs]");
    const fileParent = root.querySelector("[data-file-parent]");
    const fileBody = root.querySelector("[data-file-table-body]");
    const fileEmpty = root.querySelector("[data-file-empty]");
    const fileSnapshotAge = root.querySelector("[data-file-snapshot-age]");
    const fileMessage = root.querySelector("[data-file-message]");
    const fileRefresh = root.querySelector("[data-file-refresh]");
    const fileMkdirForm = root.querySelector("[data-file-mkdir-form]");
    const fileMkdirName = root.querySelector("[data-file-mkdir-name]");
    const fileUpload = root.querySelector("[data-file-upload]");
    const fileDownload = root.querySelector("[data-file-download]");
    const fileUploadForm = root.querySelector("[data-file-upload-form]");
    const fileUploadInput = root.querySelector("[data-file-upload-input]");
    const fileUploadDestination = root.querySelector("[data-file-upload-destination]");
    const fileTransferList = root.querySelector("[data-file-transfer-list]");
    const fileTaskLink = root.querySelector("[data-file-task-link]");
    const fileConfirm = root.querySelector("[data-file-confirm]");
    const fileConfirmText = root.querySelector("[data-file-confirm-text]");
    const fileConfirmSubmit = root.querySelector("[data-file-confirm-submit]");
    const fileConfirmCancel = root.querySelector("[data-file-confirm-cancel]");
    const fileMoveFields = root.querySelector("[data-file-move-fields]");
    const fileMoveDestination = root.querySelector("[data-file-move-destination]");
    const fileDeleteFields = root.querySelector("[data-file-delete-fields]");
    const fileDeleteRecursive = root.querySelector("[data-file-delete-recursive]");
    const browserLocation = dependencies.location || (typeof window !== "undefined" ? window.location : null);
    const browserHistory = dependencies.history || (typeof window !== "undefined" ? window.history : null);
    const browserEvents = dependencies.window || (typeof window !== "undefined" ? window : null);
    let fileRows = [];
    let fileSortKey = "name";
    let fileSortDirection = 1;
    let pendingFileAction = null;
    const transfers = new Map();

    const selectedTab = (() => {
      if (!browserLocation?.href) return "tasking";
      try {
        const value = new URL(browserLocation.href).searchParams.get("tab");
        return ["tasking", "processes", "files"].includes(value) ? value : "tasking";
      } catch (_) {
        return "tasking";
      }
    })();
    root.querySelectorAll?.("[data-callback-tab]").forEach(link => {
      link.setAttribute("aria-current", link.dataset.callbackTab === selectedTab ? "page" : "false");
    });

    const parseRemotePath = value => {
      if (typeof value !== "string" || !value || value.includes("\0")) return null;
      const drive = value.match(/^([A-Za-z]:)[\\/](.*)$/s);
      if (drive) {
        const parts = pathParts(drive[2], /[\\/]+/);
        return { root: `${drive[1]}\\`, separator: "\\", parts };
      }
      if (value.startsWith("\\\\")) {
        const all = value.slice(2).split(/[\\/]+/).filter(Boolean);
        if (all.length < 2) return null;
        const root = `\\\\${all.shift()}\\${all.shift()}`;
        return { root, separator: "\\", parts: pathParts(all.join("\\"), /[\\/]+/) };
      }
      if (value.startsWith("/")) return { root: "/", separator: "/", parts: pathParts(value.slice(1), /\/+/) };
      return null;
    };

    const pathParts = (value, separator) => {
      const parts = [];
      for (const part of value.split(separator).filter(Boolean)) {
        if (part === ".") continue;
        if (part === "..") parts.pop();
        else parts.push(part);
      }
      return parts;
    };

    const formatRemotePath = parsed => {
      if (!parsed.parts.length) return parsed.root;
      const joiner = parsed.root.endsWith(parsed.separator) ? "" : parsed.separator;
      return `${parsed.root}${joiner}${parsed.parts.join(parsed.separator)}`;
    };

    const normalizeRemotePath = value => {
      const parsed = parseRemotePath(value);
      return parsed ? formatRemotePath(parsed) : null;
    };

    const parentRemotePath = value => {
      const parsed = parseRemotePath(value);
      if (!parsed) return null;
      parsed.parts.pop();
      return formatRemotePath(parsed);
    };

    const joinRemotePath = (parent, name) => {
      const parsed = parseRemotePath(parent);
      if (!parsed || !name || name.includes("\0") || name.includes("/") || name.includes("\\")) return null;
      parsed.parts.push(name);
      return formatRemotePath(parsed);
    };

    const initialFilePath = () => {
      if (browserLocation?.href) {
        try {
          const value = new URL(browserLocation.href).searchParams.get("path");
          const normalized = normalizeRemotePath(value || "");
          if (normalized) return normalized;
        } catch (_) { /* use callback platform root */ }
      }
      return normalizeRemotePath(fileDataset.defaultPath || "/") || "/";
    };

    let currentFilePath = initialFilePath();

    const element = (tag, className, text) => {
      const node = doc.createElement(tag);
      if (className) node.className = className;
      if (text !== undefined && text !== null) node.textContent = String(text);
      return node;
    };

    const unavailable = value => value === null || value === undefined || value === "" ? "Unavailable" : value;

    const processAge = capturedAt => {
      const captured = Date.parse(capturedAt || "");
      if (!Number.isFinite(captured)) return "Snapshot time unavailable";
      const current = typeof dependencies.now === "function" ? dependencies.now() : Date.now();
      const ageMs = Math.max(0, current - captured);
      const minutes = Math.floor(ageMs / 60000);
      const hours = Math.floor(minutes / 60);
      const days = Math.floor(hours / 24);
      const remainderHours = hours % 24;
      const remainderMinutes = minutes % 60;
      let age = days ? `${days}d ${remainderHours}h` : hours ? `${hours}h ${remainderMinutes}m` : `${remainderMinutes}m`;
      return `${age} old${ageMs > 15 * 60000 ? " · stale" : ""}`;
    };

    const setFileTaskLink = task => {
      const taskId = task?.id || task?.task_id;
      if (!fileTaskLink || !taskId) return;
      fileTaskLink.setAttribute("href", `#task-${taskId}`);
      fileTaskLink.textContent = `Task ${taskId}`;
      fileTaskLink.hidden = false;
    };

    const renderFileNavigation = () => {
      if (filePathInput) filePathInput.value = currentFilePath;
      const parsed = parseRemotePath(currentFilePath);
      if (!parsed) return;
      const crumbs = [];
      const root = element("button", "file-crumb", parsed.root);
      root.type = "button";
      root.dataset.filePath = parsed.root;
      crumbs.push(root);
      const accumulated = [];
      for (const part of parsed.parts) {
        accumulated.push(part);
        const target = formatRemotePath({ ...parsed, parts: [...accumulated] });
        const separator = element("span", "file-crumb-separator", parsed.separator);
        const button = element("button", "file-crumb", part);
        button.type = "button";
        button.dataset.filePath = target;
        crumbs.push(separator, button);
      }
      fileBreadcrumbs?.replaceChildren(...crumbs);
      if (fileParent) {
        const parent = parentRemotePath(currentFilePath);
        fileParent.dataset.filePath = parent;
        fileParent.disabled = parent === currentFilePath;
      }
    };

    const updateFileUrl = () => {
      if (!browserLocation?.href || !browserHistory?.pushState) return;
      try {
        const url = new URL(browserLocation.href);
        url.searchParams.set("tab", "files");
        url.searchParams.set("path", currentFilePath);
        url.hash = "files";
        browserHistory.pushState({}, "", `${url.pathname}?${url.searchParams.toString()}${url.hash}`);
      } catch (_) { /* navigation still works without URL state */ }
    };

    const fileValue = (entry, key) => {
      if (key === "size") return entry.size === null || entry.size === undefined ? "Unavailable" : `${entry.size} bytes`;
      return unavailable(entry[key]);
    };

    const renderFiles = () => {
      if (!fileBody) return;
      const sorted = [...fileRows].sort((left, right) => {
        if (fileSortKey === "name" && left.kind !== right.kind) {
          if (left.kind === "directory") return -1;
          if (right.kind === "directory") return 1;
        }
        const leftValue = left[fileSortKey];
        const rightValue = right[fileSortKey];
        if (typeof leftValue === "number" && typeof rightValue === "number") return (leftValue - rightValue) * fileSortDirection;
        return String(leftValue ?? "").localeCompare(String(rightValue ?? "")) * fileSortDirection;
      });
      const rows = sorted.map(entry => {
        const row = element("tr", "file-row");
        row.dataset.fileRow = "";
        row.dataset.filePath = String(entry.path);
        row.dataset.fileKind = String(entry.kind);
        for (const key of ["name", "kind", "size", "modified_at", "permissions", "owner"]) {
          const cell = element("td", "", fileValue(entry, key));
          cell.dataset.fileField = key;
          row.append(cell);
        }
        const actions = element("td", "file-actions");
        const move = element("button", "btn btn-ghost", "Move / rename");
        move.type = "button";
        move.dataset.fileMove = "";
        const remove = element("button", "btn btn-ghost", "Delete");
        remove.type = "button";
        remove.dataset.fileDelete = "";
        actions.append(move, remove);
        if (entry.kind === "file" && fileDataset.transferCapable === "true") {
          const download = element("button", "btn btn-ghost", "Download");
          download.type = "button";
          download.dataset.fileDownloadAction = "";
          actions.prepend(download);
        }
        row.append(actions);
        return row;
      });
      fileBody.replaceChildren(...rows);
      if (fileEmpty) fileEmpty.hidden = rows.length !== 0;
    };

    const renderTransfers = () => {
      if (!fileTransferList) return;
      const items = [...transfers.values()].map(transfer => {
        const total = Number(transfer.expected_size) || 0;
        const received = Math.max(0, Number(transfer.received_bytes) || 0);
        const percent = total ? Math.min(100, Math.floor((received / total) * 100)) : 0;
        const item = element("article", "file-transfer");
        item.dataset.transferId = String(transfer.id);
        item.append(
          element("strong", "file-transfer-path", unavailable(transfer.remote_path)),
          element("span", "file-transfer-direction", unavailable(transfer.direction)),
          element("span", "file-transfer-progress", `${received} / ${total || "?"} bytes · ${percent}% · ${unavailable(transfer.status)}`),
          element("code", "file-transfer-checksum", transfer.sha256 ? `SHA-256 ${transfer.sha256}` : "SHA-256 pending"),
        );
        if (transfer.error) item.append(element("p", "file-transfer-error", transfer.error));
        if (transfer.direction === "download" && transfer.status === "completed") {
          const link = element("a", "btn btn-ghost", "Save verified file");
          link.setAttribute("href", `${fileDataset.transfersEndpoint}/${encodeURIComponent(transfer.id)}/download`);
          item.append(link);
        }
        return item;
      });
      fileTransferList.replaceChildren(...items);
    };

    const upsertTransfer = transfer => {
      if (!transfer?.id) return;
      transfers.set(transfer.id, { ...(transfers.get(transfer.id) || {}), ...transfer });
      renderTransfers();
    };

    const queueDownload = async entry => {
      const response = await request(`${fileDataset.filesEndpoint}/download`, {
        method: "POST",
        credentials: "same-origin",
        headers: { "content-type": "application/json", "x-csrf-token": root.dataset.csrfToken },
        body: JSON.stringify({ path: entry.path, expected_size: entry.size }),
      });
      if (!response.ok) throw new Error(`Download request failed (${response.status})`);
      const transfer = await response.json();
      upsertTransfer(transfer);
      if (fileMessage) fileMessage.textContent = `Download transfer ${transfer.id} queued.`;
      return transfer;
    };

    const loadFiles = async () => {
      if (!filePanel || fileDataset.fileCapable !== "true") return;
      const response = await request(`${fileDataset.filesEndpoint}?path=${encodeURIComponent(currentFilePath)}`, { credentials: "same-origin" });
      if (!response.ok) throw new Error(`Filesystem snapshot request failed (${response.status})`);
      const snapshot = await response.json();
      const data = snapshot?.snapshot_json || snapshot;
      fileRows = Array.isArray(data?.entries) ? data.entries : [];
      if (fileSnapshotAge) fileSnapshotAge.textContent = data?.captured_at ? processAge(data.captured_at) : "No filesystem snapshot yet";
      setFileTaskLink(snapshot);
      renderFiles();
    };

    const navigateFiles = async (path, updateUrl = true) => {
      const normalized = normalizeRemotePath(path);
      if (!normalized) {
        if (fileMessage) fileMessage.textContent = "Enter an absolute Linux, Windows drive, or UNC path.";
        return;
      }
      currentFilePath = normalized;
      renderFileNavigation();
      if (updateUrl) updateFileUrl();
      try {
        await loadFiles();
        if (fileMessage) fileMessage.textContent = fileRows.length ? "Filesystem snapshot loaded." : "No snapshot for this path; queue a refresh.";
      } catch (_) {
        if (fileMessage) fileMessage.textContent = "Filesystem snapshot unavailable; Tasking remains connected.";
      }
    };

    const queueFilesystemTask = async (route, body) => {
      const response = await request(`${fileDataset.filesEndpoint}/${route}`, {
        method: "POST",
        credentials: "same-origin",
        headers: { "content-type": "application/json", "x-csrf-token": root.dataset.csrfToken },
        body: JSON.stringify(body),
      });
      if (!response.ok) throw new Error(`Filesystem task failed (${response.status})`);
      const task = await response.json();
      upsert(task);
      setFileTaskLink(task);
      return task;
    };

    const updateFileConfirmation = () => {
      if (!pendingFileAction || !fileConfirmText) return;
      if (pendingFileAction.kind === "mkdir") {
        fileConfirmText.textContent = `Mkdir exact remote path ${pendingFileAction.path}?`;
      } else if (pendingFileAction.kind === "move") {
        const destination = fileMoveDestination?.value || "";
        fileConfirmText.textContent = `Move exact source ${pendingFileAction.source} to exact destination ${destination}?`;
      } else {
        const recursive = Boolean(fileDeleteRecursive?.checked);
        fileConfirmText.textContent = `Delete exact remote path ${pendingFileAction.path}${recursive ? " recursively" : " non-recursively"}?`;
      }
    };

    const openFileConfirmation = action => {
      pendingFileAction = action;
      if (fileMoveFields) fileMoveFields.hidden = action.kind !== "move";
      if (fileDeleteFields) fileDeleteFields.hidden = action.kind !== "delete";
      if (action.kind === "move" && fileMoveDestination) fileMoveDestination.value = action.source;
      if (action.kind === "delete" && fileDeleteRecursive) fileDeleteRecursive.checked = false;
      updateFileConfirmation();
      if (fileConfirm) fileConfirm.hidden = false;
    };

    const restoreFileLocation = async () => {
      if (!browserLocation?.href) return;
      try {
        const url = new URL(browserLocation.href);
        const path = normalizeRemotePath(url.searchParams.get("path") || "");
        root.querySelectorAll?.("[data-callback-tab]").forEach(link => {
          link.setAttribute("aria-current", link.dataset.callbackTab === url.searchParams.get("tab") ? "page" : "false");
        });
        if (path && path !== currentFilePath) await navigateFiles(path, false);
      } catch (_) { /* ignore malformed browser history entries */ }
    };

    const processValue = (process, key) => {
      if (key === "pid") return process.pid;
      if (key === "parent_pid") return unavailable(process.parent_pid);
      if (key === "cpu_percent") return process.cpu_percent === null || process.cpu_percent === undefined ? "Unavailable" : `${process.cpu_percent}%`;
      if (key === "memory_bytes") return process.memory_bytes === null || process.memory_bytes === undefined ? "Unavailable" : `${process.memory_bytes} bytes`;
      return unavailable(process[key]);
    };

    const renderProcessDetail = process => {
      if (!processDetail || !process) return;
      processDetail.replaceChildren(
        element("strong", "process-detail-name", unavailable(process.name)),
        element("p", "process-detail-pid", `PID ${process.pid}`),
        element("p", "", `Parent PID: ${processValue(process, "parent_pid")}`),
        element("p", "", `Executable: ${unavailable(process.executable)}`),
        element("p", "", `User: ${unavailable(process.user)}`),
        element("p", "", `Architecture: ${unavailable(process.architecture)}`),
        element("p", "", `CPU: ${processValue(process, "cpu_percent")}`),
        element("p", "", `Memory: ${processValue(process, "memory_bytes")}`),
        element("p", "", `Started: ${unavailable(process.started_at)}`),
      );
    };

    const renderProcesses = () => {
      if (!processBody) return;
      const query = (processSearch?.value || "").trim().toLowerCase();
      const sorted = [...processRows].sort((a, b) => {
        const left = processValue(a, processSortKey);
        const right = processValue(b, processSortKey);
        if (typeof left === "number" && typeof right === "number") return (left - right) * processSortDirection;
        return String(left).localeCompare(String(right)) * processSortDirection;
      });
      const rows = sorted.map(process => {
        const row = element("tr", "process-row");
        row.dataset.processRow = "";
        row.dataset.processPid = String(process.pid);
        const haystack = [process.pid, process.parent_pid, process.name, process.executable, process.user, process.architecture]
          .filter(value => value !== null && value !== undefined).join(" ").toLowerCase();
        row.hidden = Boolean(query && !haystack.includes(query));
        for (const key of ["pid", "parent_pid", "name", "executable", "user", "architecture", "cpu_percent", "memory_bytes", "started_at"]) {
          const cell = element("td", "", processValue(process, key));
          cell.dataset.processField = key;
          row.append(cell);
        }
        const actions = element("td", "process-actions");
        const kill = element("button", "btn btn-ghost", "Kill");
        kill.type = "button";
        kill.dataset.processKill = "";
        actions.append(kill);
        row.append(actions);
        return row;
      });
      processBody.replaceChildren(...rows);
      if (processEmpty) processEmpty.hidden = rows.some(row => !row.hidden);
    };

    const loadProcesses = async () => {
      if (!processPanel || processDataset.processCapable !== "true") return;
      const response = await request(processDataset.processesEndpoint, { credentials: "same-origin" });
      if (!response.ok) throw new Error(`Process snapshot request failed (${response.status})`);
      const snapshot = await response.json();
      const data = snapshot?.snapshot_json || snapshot;
      processRows = Array.isArray(data?.processes) ? data.processes : [];
      if (processSnapshotAge) processSnapshotAge.textContent = data?.captured_at ? processAge(data.captured_at) : "No process snapshot yet";
      if (processTaskLink && snapshot?.task_id) {
        processTaskLink.setAttribute("href", `#task-${snapshot.task_id}`);
        processTaskLink.textContent = `Task ${snapshot.task_id}`;
        processTaskLink.hidden = false;
      }
      renderProcesses();
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
      card.id = `task-${task.id}`;
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

    const upsert = (task, options = {}) => {
      if (!task || !task.id) return;
      const previous = tasks.get(task.id);
      tasks.set(task.id, { ...(tasks.get(task.id) || {}), ...task });
      let card = cards.get(task.id);
      if (!card) {
        card = createCard(task);
        cards.set(task.id, card);
      }
      updateCard(card, tasks.get(task.id));
      renderOrder();
      if ((options.fromEvent || previous) && previous?.status !== "completed" && task.status === "completed" && task.command === "nw/process-list") {
        void loadProcesses().catch(() => {
          if (processMessage) processMessage.textContent = "Process snapshot could not be refreshed; retry when the callback is available.";
        });
      }
      if ((options.fromEvent || previous) && previous?.status !== "completed" && task.status === "completed" && task.command === "nw/fs-list") {
        void loadFiles().catch(() => {
          if (fileMessage) fileMessage.textContent = "Filesystem snapshot could not be refreshed; retry when the callback is available.";
        });
      }
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
        try { upsert(JSON.parse(event.data), { fromEvent: true }); } catch (_) { /* ignore malformed snapshots */ }
      });
      eventSource.addEventListener("transfer", event => {
        try { upsertTransfer(JSON.parse(event.data)); } catch (_) { /* ignore malformed snapshots */ }
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

    processSearch?.addEventListener("input", renderProcesses);
    root.querySelectorAll?.("[data-process-sort]").forEach(button => {
      button.addEventListener("click", () => {
        const key = button.dataset.processSort;
        if (processSortKey === key) processSortDirection *= -1;
        else {
          processSortKey = key;
          processSortDirection = -1;
        }
        renderProcesses();
      });
    });
    processBody?.addEventListener("click", event => {
      const kill = event.target.closest?.("[data-process-kill]");
      const row = event.target.closest?.("[data-process-row]");
      if (!row) return;
      const process = processRows.find(candidate => String(candidate.pid) === row.dataset.processPid);
      if (!process) return;
      selectedProcess = process;
      renderProcessDetail(process);
      if (kill && processConfirm && processConfirmText) {
        processConfirmText.textContent = `Terminate ${unavailable(process.name)} (PID ${process.pid})? This exact process will be targeted.`;
        processConfirm.hidden = false;
      }
    });
    processRefresh?.addEventListener("click", async () => {
      if (processRefresh.disabled) return;
      processRefresh.disabled = true;
      try {
        const response = await request(`${processDataset.processesEndpoint}/refresh`, {
          method: "POST",
          credentials: "same-origin",
          headers: { "x-csrf-token": root.dataset.csrfToken },
        });
        if (!response.ok) throw new Error(`Process refresh task failed (${response.status})`);
        const task = await response.json();
        upsert(task);
        if (processTaskLink) {
          processTaskLink.setAttribute("href", `#task-${task.id}`);
          processTaskLink.textContent = `Task ${task.id}`;
          processTaskLink.hidden = false;
        }
        if (processMessage) processMessage.textContent = "Process refresh queued.";
      } catch (_) {
        if (processMessage) processMessage.textContent = "Unable to queue process refresh; check callback scope and connection.";
      } finally {
        processRefresh.disabled = false;
      }
    });
    processConfirmCancel?.addEventListener("click", () => {
      if (processConfirm) processConfirm.hidden = true;
      selectedProcess = null;
    });
    processConfirmSubmit?.addEventListener("click", async () => {
      if (!selectedProcess || processConfirmSubmit.disabled) return;
      const target = selectedProcess;
      processConfirmSubmit.disabled = true;
      try {
        const response = await request(`${processDataset.processesEndpoint}/${encodeURIComponent(target.pid)}/kill`, {
          method: "POST",
          credentials: "same-origin",
          headers: { "x-csrf-token": root.dataset.csrfToken },
        });
        if (!response.ok) throw new Error(`Process kill task failed (${response.status})`);
        const task = await response.json();
        upsert(task);
        if (processTaskLink) {
          processTaskLink.setAttribute("href", `#task-${task.id}`);
          processTaskLink.textContent = `Task ${task.id}`;
          processTaskLink.hidden = false;
        }
        if (processMessage) processMessage.textContent = `Kill task ${task.id} queued; the table refreshes after successful completion.`;
        if (processConfirm) processConfirm.hidden = true;
        selectedProcess = null;
      } catch (_) {
        if (processMessage) processMessage.textContent = "Unable to queue process kill; check callback scope and connection.";
      } finally {
        processConfirmSubmit.disabled = false;
      }
    });

    filePathForm?.addEventListener("submit", async event => {
      event.preventDefault();
      await navigateFiles(filePathInput?.value || "");
    });
    fileParent?.addEventListener("click", async () => {
      if (fileParent.disabled) return;
      await navigateFiles(fileParent.dataset.filePath);
    });
    fileBreadcrumbs?.addEventListener("click", async event => {
      const button = event.target.closest?.("[data-file-path]");
      if (button) await navigateFiles(button.dataset.filePath);
    });
    root.querySelectorAll?.("[data-file-sort]").forEach(button => {
      button.addEventListener("click", () => {
        const key = button.dataset.fileSort;
        if (fileSortKey === key) fileSortDirection *= -1;
        else {
          fileSortKey = key;
          fileSortDirection = -1;
        }
        renderFiles();
      });
    });
    fileBody?.addEventListener("click", async event => {
      const row = event.target.closest?.("[data-file-row]");
      if (!row) return;
      if (event.target.closest?.("[data-file-download-action]")) {
        try {
          const entry = fileRows.find(candidate => String(candidate.path) === row.dataset.filePath);
          if (entry) await queueDownload(entry);
        } catch (_) {
          if (fileMessage) fileMessage.textContent = "Unable to queue download; check callback scope and retry.";
        }
      } else if (event.target.closest?.("[data-file-move]")) {
        openFileConfirmation({ kind: "move", source: row.dataset.filePath });
      } else if (event.target.closest?.("[data-file-delete]")) {
        openFileConfirmation({ kind: "delete", path: row.dataset.filePath });
      } else if (row.dataset.fileKind === "directory") {
        await navigateFiles(row.dataset.filePath);
      }
    });
    fileRefresh?.addEventListener("click", async () => {
      if (fileRefresh.disabled) return;
      fileRefresh.disabled = true;
      try {
        const task = await queueFilesystemTask("list", { path: currentFilePath });
        if (fileMessage) fileMessage.textContent = `Filesystem refresh task ${task.id} queued.`;
      } catch (_) {
        if (fileMessage) fileMessage.textContent = "Unable to queue filesystem refresh; check callback scope and connection.";
      } finally {
        fileRefresh.disabled = false;
      }
    });
    fileMkdirForm?.addEventListener("submit", event => {
      event.preventDefault();
      const path = joinRemotePath(currentFilePath, fileMkdirName?.value || "");
      if (!path) {
        if (fileMessage) fileMessage.textContent = "Directory name must be one path component.";
        return;
      }
      openFileConfirmation({ kind: "mkdir", path });
    });
    fileMoveDestination?.addEventListener("input", updateFileConfirmation);
    fileDeleteRecursive?.addEventListener("change", updateFileConfirmation);
    fileConfirmCancel?.addEventListener("click", () => {
      pendingFileAction = null;
      if (fileConfirm) fileConfirm.hidden = true;
    });
    fileConfirmSubmit?.addEventListener("click", async () => {
      if (!pendingFileAction || fileConfirmSubmit.disabled) return;
      let route;
      let body;
      if (pendingFileAction.kind === "mkdir") {
        route = "mkdir";
        body = { path: pendingFileAction.path };
      } else if (pendingFileAction.kind === "move") {
        const destination = normalizeRemotePath(fileMoveDestination?.value || "");
        if (!destination) {
          if (fileMessage) fileMessage.textContent = "Move destination must be an absolute remote path.";
          return;
        }
        route = "move";
        body = { source: pendingFileAction.source, destination };
      } else {
        route = "delete";
        body = { path: pendingFileAction.path, recursive: Boolean(fileDeleteRecursive?.checked) };
      }
      fileConfirmSubmit.disabled = true;
      try {
        const task = await queueFilesystemTask(route, body);
        if (fileMessage) fileMessage.textContent = `Filesystem ${route} task ${task.id} queued.`;
        if (fileConfirm) fileConfirm.hidden = true;
        pendingFileAction = null;
        if (fileMkdirName) fileMkdirName.value = "";
      } catch (_) {
        if (fileMessage) fileMessage.textContent = `Unable to queue filesystem ${route}; check callback scope and connection.`;
      } finally {
        fileConfirmSubmit.disabled = false;
      }
    });
    fileUploadForm?.addEventListener("submit", async event => {
      event.preventDefault();
      if (fileDataset.transferCapable !== "true" || fileUpload?.disabled) return;
      const destination = normalizeRemotePath(fileUploadDestination?.value || "");
      const file = fileUploadInput?.files?.[0];
      if (!destination || !file) {
        if (fileMessage) fileMessage.textContent = "Choose one local file and an absolute remote destination.";
        return;
      }
      fileUpload.disabled = true;
      try {
        const FormDataType = dependencies.FormData || FormData;
        const body = new FormDataType();
        body.append("destination", destination);
        body.append("file", file);
        const response = await request(`${fileDataset.filesEndpoint}/upload`, {
          method: "POST",
          credentials: "same-origin",
          headers: { "x-csrf-token": root.dataset.csrfToken },
          body,
        });
        if (!response.ok) throw new Error(`Upload request failed (${response.status})`);
        const transfer = await response.json();
        upsertTransfer(transfer);
        if (fileMessage) fileMessage.textContent = `Upload transfer ${transfer.id} queued.`;
      } catch (_) {
        if (fileMessage) fileMessage.textContent = "Unable to stage upload; verify size, scope, and destination.";
      } finally {
        fileUpload.disabled = false;
      }
    });
    fileDownload?.addEventListener("click", () => {
      if (fileMessage) fileMessage.textContent = "Choose Download beside an exact remote file.";
    });
    browserEvents?.addEventListener?.("popstate", restoreFileLocation);

    if (offlineQueue) {
      offlineQueue.hidden = root.dataset.online !== "false";
      offlineQueue.textContent = "Callback is offline. New tasks stay queued until its next check-in.";
    }
    if (processPanel) {
      const capable = processDataset.processCapable === "true";
      if (!capable) {
        if (processRefresh) processRefresh.disabled = true;
        if (processMessage) processMessage.textContent = "This callback does not advertise process control capability.";
      } else if (root.dataset.online === "false" && processMessage) {
        processMessage.textContent = "Callback is offline. Process tasks remain queued until its next check-in.";
      }
    }
    if (filePanel) {
      const capable = fileDataset.fileCapable === "true";
      renderFileNavigation();
      if (fileUpload) {
        fileUpload.disabled = fileDataset.transferCapable !== "true";
        fileUpload.textContent = fileDataset.transferCapable === "true" ? "Upload file" : "Transfer support is being initialized";
      }
      if (fileDownload) {
        fileDownload.disabled = fileDataset.transferCapable !== "true";
        fileDownload.textContent = fileDataset.transferCapable === "true" ? "Download a listed file" : "Transfer support is being initialized";
      }
      if (!capable) {
        if (fileRefresh) fileRefresh.disabled = true;
        if (fileMessage) fileMessage.textContent = "This callback does not advertise filesystem control capability.";
      } else if (root.dataset.online === "false" && fileMessage) {
        fileMessage.textContent = "Callback is offline. Filesystem tasks remain queued until its next check-in.";
      }
    }
    setConnection("Connecting");
    const ready = loadPage()
      .then(() => {
        if (closed) return undefined;
        return loadProcesses().catch(() => {
          if (processMessage) processMessage.textContent = "Process snapshot unavailable; Tasking remains connected.";
        });
      })
      .then(() => {
        if (closed) return undefined;
        return loadFiles().catch(() => {
          if (fileMessage) fileMessage.textContent = "Filesystem snapshot unavailable; Tasking remains connected.";
        });
      })
      .then(() => connect())
      .catch(() => setConnection("History unavailable — retry"));

    return {
      ready,
      upsert,
      close() {
        if (closed) return;
        closed = true;
        browserEvents?.removeEventListener?.("popstate", restoreFileLocation);
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
