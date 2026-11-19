"use strict";

const byId = id => document.getElementById(id);
const whole = new Intl.NumberFormat("en", { maximumFractionDigits: 0 });
const axisNumber = new Intl.NumberFormat("en", { maximumFractionDigits: 3 });
const shortTime = new Intl.DateTimeFormat("en", { hour: "numeric", minute: "2-digit", second: "2-digit" });
const windowTime = new Intl.DateTimeFormat("en", {
  month: "short", day: "numeric", hour: "numeric", minute: "2-digit", second: "2-digit",
});

const history = { page: null, before: null, previous: [], loading: false, failed: false };
const engine = { process: null, action: null, statusRequest: 0, logsLoading: false };
const durationSeries = [
  { field: "mean_ns", color: "--mean", dash: [] },
  { field: "p95_ns", color: "--p95", dash: [6, 4] },
  { field: "p99_ns", color: "--p99", dash: [2, 4] },
];
const colors = getComputedStyle(document.documentElement);
const color = name => colors.getPropertyValue(name).trim();

function setText(id, value) {
  const node = byId(id);
  if (node.textContent !== value) node.textContent = value;
}

function count(value) {
  return Number.isFinite(value) ? whole.format(value) : "—";
}

function milliseconds(ns) {
  if (!Number.isFinite(ns)) return "—";
  if (ns > 0 && ns < 10_000) return "<0.01 ms";
  return (ns / 1_000_000).toFixed(2) + " ms";
}

function elapsed(ms) {
  return Number.isFinite(ms) ? (ms / 1000).toFixed(1) + " s" : "—";
}

function showError(id, message) {
  setText(id, message);
  byId(id).hidden = !message;
}

async function request(path, options = {}) {
  const response = await fetch(path, { cache: "no-store", ...options });
  const isJson = response.headers.get("content-type")?.startsWith("application/json");
  const body = isJson ? await response.json() : await response.text();
  if (!response.ok) {
    throw new Error((isJson ? body.error : body) || "Request failed.");
  }
  return body;
}

function populateTable(id, rows, columnCount, emptyMessage) {
  const fragment = document.createDocumentFragment();
  for (const row of rows) {
    const tr = document.createElement("tr");
    for (const value of row) {
      const td = document.createElement("td");
      td.textContent = value;
      tr.append(td);
    }
    fragment.append(tr);
  }
  if (rows.length === 0) {
    const tr = document.createElement("tr");
    const td = document.createElement("td");
    td.colSpan = columnCount;
    td.textContent = emptyMessage;
    tr.append(td);
    fragment.append(tr);
  }
  byId(id).replaceChildren(fragment);
}

function renderHistory() {
  const samples = history.page?.samples ?? [];
  const latest = samples.at(-1);
  setText("mean", milliseconds(latest?.mean_ns));
  setText("p95", milliseconds(latest?.p95_ns));
  setText("p99", milliseconds(latest?.p99_ns));
  setText("skipped", count(latest?.skipped_ticks));

  if (latest) {
    const label = history.before === null ? "Latest window" : "Last window on this page";
    setText("window-label", label + " · " + elapsed(latest.window_ms) +
      " ending " + windowTime.format(latest.ended_at_ms));
    setText("range-label", windowTime.format(samples[0].ended_at_ms) +
      " – " + windowTime.format(latest.ended_at_ms) + " · local time");
  } else {
    setText("window-label", history.before === null ? "Waiting for the first saved window…" : "No saved windows on this page");
    setText("range-label", "");
  }
  populateTable("table-body", [...samples].reverse().map(sample => [
    windowTime.format(sample.ended_at_ms),
    elapsed(sample.window_ms),
    count(sample.ticks),
    milliseconds(sample.mean_ns),
    milliseconds(sample.p95_ns),
    milliseconds(sample.p99_ns),
    milliseconds(sample.max_ns),
    count(sample.skipped_ticks),
    count(sample.lagging_iterations),
  ]), 9, "No saved windows.");
  drawCharts();
}

function historyControls() {
  byId("older").disabled = history.loading || !history.page?.next_before;
  byId("newer").disabled = history.loading || history.previous.length === 0;
  byId("latest").disabled = history.loading || (history.before === null && !history.failed);
}

async function loadHistory(before = null, previous = []) {
  if (history.loading) return;
  history.loading = true;
  historyControls();
  try {
    const query = before === null ? "" : "?before=" + encodeURIComponent(before);
    const page = await request("/api/metrics" + query, { signal: AbortSignal.timeout(4500) });
    if (!Array.isArray(page.samples)) throw new Error("Invalid history data.");
    history.page = page;
    history.before = before;
    history.previous = previous;
    history.failed = false;
    showError("error", "");
    renderHistory();
  } catch (error) {
    history.failed = true;
    showError("error", error.message || "Could not load tick history.");
  } finally {
    history.loading = false;
    historyControls();
  }
}

function plot(canvas, samples, step) {
  const { width, height } = canvas.getBoundingClientRect();
  const ctx = canvas.getContext("2d");
  if (!ctx || !width || !height) return null;
  const ratio = window.devicePixelRatio || 1;
  canvas.width = Math.round(width * ratio);
  canvas.height = Math.round(height * ratio);
  ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
  ctx.font = "11px system-ui, sans-serif";
  ctx.fillStyle = color("--muted");
  ctx.strokeStyle = color("--line");
  ctx.lineWidth = 1;
  if (samples.length === 0) {
    ctx.textAlign = "center";
    ctx.fillText("No measurements", width / 2, height / 2);
    return null;
  }

  const times = samples.map(sample => sample.ended_at_ms);
  const first = Math.min(...times);
  const last = Math.max(...times);
  const left = Math.max(38, Math.ceil(ctx.measureText(axisNumber.format(step * 4)).width) + 12);
  const right = width - 8;
  const top = 8;
  const bottom = height - 25;
  const x = ms => first === last ? (left + right) / 2 :
    left + (ms - first) / (last - first) * (right - left);
  const y = value => bottom - value / (step * 4) * (bottom - top);

  ctx.textAlign = "right";
  for (let line = 0; line <= 4; line += 1) {
    const value = step * line;
    const at = y(value);
    ctx.beginPath();
    ctx.moveTo(left, at);
    ctx.lineTo(right, at);
    ctx.stroke();
    ctx.fillText(axisNumber.format(value), left - 6, at + 4);
  }
  const time = last - first >= 86_400_000 ? windowTime : shortTime;
  ctx.textAlign = first === last ? "center" : "left";
  ctx.fillText(time.format(first), x(first), height - 5);
  if (first !== last) {
    ctx.textAlign = "right";
    ctx.fillText(time.format(last), x(last), height - 5);
  }
  return { ctx, x, y, left, right, first, last };
}

function drawDuration(samples) {
  const canvas = byId("duration-chart");
  const max = Math.max(0, ...samples.flatMap(sample => durationSeries.map(series =>
    (sample[series.field] ?? 0) / 1_000_000)));
  const rawStep = Math.max(.001, max / 4);
  const unit = 10 ** Math.floor(Math.log10(rawStep));
  const step = Math.ceil(rawStep / unit) * unit;
  const chart = plot(canvas, samples, step);
  if (!chart) return;
  const { ctx, x, y } = chart;

  for (const { field, color: stroke, dash } of durationSeries) {
    ctx.strokeStyle = ctx.fillStyle = color(stroke);
    ctx.lineWidth = 1.8;
    ctx.setLineDash(dash);
    ctx.beginPath();
    let last = null;
    for (const sample of samples) {
      const ns = sample[field];
      if (!Number.isFinite(ns)) {
        last = null;
        continue;
      }
      const fromLast = last === null ? 0 : sample.ended_at_ms - last.ended_at_ms;
      const gap = fromLast <= 0 || fromLast > 2.5 * Math.max(sample.window_ms, last?.window_ms ?? 0);
      const t = x(sample.ended_at_ms);
      const work = y(ns / 1_000_000);
      if (gap) ctx.moveTo(t, work);
      else ctx.lineTo(t, work);
      last = sample;
    }
    ctx.stroke();
    ctx.setLineDash([]);
    for (const sample of samples) {
      if (Number.isFinite(sample[field])) {
        ctx.beginPath();
        ctx.arc(x(sample.ended_at_ms), y(sample[field] / 1_000_000), samples.length > 60 ? 1.1 : 2.1, 0, Math.PI * 2);
        ctx.fill();
      }
    }
  }
}

function drawSkips(samples) {
  const max = Math.max(0, ...samples.map(sample => sample.skipped_ticks));
  const chart = plot(byId("skips-chart"), samples, Math.max(1, Math.ceil(max / 4)));
  if (!chart) return;
  const { ctx, x, y, left, right, first, last } = chart;
  ctx.fillStyle = color("--p99");
  for (const sample of samples) {
    if (sample.skipped_ticks === 0) continue;
    const proportional = last === first ? 14 :
      sample.window_ms / (last - first) * (right - left) * .72;
    const width = Math.max(2, Math.min(16, proportional));
    const start = Math.max(left, Math.min(right - width, x(sample.ended_at_ms) - width / 2));
    ctx.fillRect(start, y(sample.skipped_ticks), width, y(0) - y(sample.skipped_ticks));
  }
}

function drawCharts() {
  const samples = history.page?.samples ?? [];
  drawDuration(samples);
  drawSkips(samples);
}

function renderControls() {
  const known = typeof engine.process?.running === "boolean";
  const busy = engine.action !== null;
  byId("start").disabled = busy || !known || engine.process.running;
  byId("stop").disabled = busy || !known || !engine.process.running;
  const state = !known ? "unavailable" : engine.process.running ? "live" : "stopped";
  byId("status").dataset.state = state;
  setText("status", busy ? (engine.action === "start" ? "Starting…" : "Stopping…") :
    state === "live" ? "Running" : state === "stopped" ? "Stopped" : "Unavailable");
}

function renderEngine(data) {
  engine.process = data.process;
  const running = data.process?.running;
  const pid = data.process?.pid;
  const exit = data.process?.last_exit;
  if (running) setText("process-detail", pid ? "Running · PID " + pid : "Running");
  else setText("process-detail", exit == null ? "Engine is stopped." : "Engine is stopped · " +
    (typeof exit === "string" ? exit : "Last exit " + JSON.stringify(exit)));
  renderControls();

  const saved = data.engine;
  setText("engine-version", saved?.core_version ?? "—");
  setText("engine-rate", Number.isFinite(saved?.tick_rate) ? saved.tick_rate + " Hz" : "—");
  const extension = saved?.extension;
  setText("engine-extension", extension?.id ? extension.id + (extension.version ? " " + extension.version : "") : "—");
  const modules = Array.isArray(saved?.modules) ? saved.modules : [];
  byId("modules").replaceChildren(...modules.map(name => {
    const li = document.createElement("li");
    li.textContent = name;
    return li;
  }));
  setText("engine-note", saved ? (running ? "" : "Last recorded engine configuration.") :
    (running ? "Waiting for the first recorded configuration." : "Start the engine to record its configuration."));

  const mechanics = Array.isArray(saved?.mechanics) ? [...saved.mechanics] : [];
  mechanics.sort((left, right) => right.total_ns - left.total_ns);
  populateTable("mechanics-body", mechanics.map(row => [
    row.name, row.phase, count(row.calls), milliseconds(row.total_ns), milliseconds(row.max_ns),
  ]), 5, "No timing samples for mechanics yet.");
  const recorded = Number.isFinite(saved?.ended_at_ms) ? " · " + windowTime.format(saved.ended_at_ms) : "";
  setText("mechanics-note", mechanics.length ? "Latest recorded window" + recorded :
    "Waiting for a saved timing window…");
}

async function loadStatus() {
  if (engine.action !== null) return;
  const requestId = ++engine.statusRequest;
  try {
    const status = await request("/api/status", { signal: AbortSignal.timeout(4500) });
    if (requestId === engine.statusRequest) renderEngine(status);
  } catch {
    if (requestId === engine.statusRequest) {
      engine.process = null;
      setText("process-detail", "Could not read the engine status.");
      renderControls();
    }
  }
}

async function loadLogs() {
  if (engine.logsLoading) return;
  engine.logsLoading = true;
  byId("refresh-logs").disabled = true;
  try {
    const data = await request("/api/logs", { signal: AbortSignal.timeout(4500) });
    const logs = byId("log-content");
    const follow = logs.scrollHeight - logs.scrollTop - logs.clientHeight < 30;
    logs.textContent = data.logs || "No server output yet.";
    if (follow) logs.scrollTop = logs.scrollHeight;
    setText("logs-note", "Updated " + shortTime.format(Date.now()));
  } catch (error) {
    setText("logs-note", error.message || "Could not load logs.");
  } finally {
    engine.logsLoading = false;
    byId("refresh-logs").disabled = false;
  }
}

async function control(action) {
  if (engine.action !== null) return;
  engine.action = action;
  engine.statusRequest += 1;
  renderControls();
  showError("process-error", "");
  try {
    await request("/api/" + action, {
      method: "POST",
      headers: { "X-Crucible-Admin": "1" },
    });
  } catch (error) {
    showError("process-error", error.message || "The process command failed.");
  } finally {
    engine.action = null;
    await loadStatus();
    await loadLogs();
  }
}

byId("start").addEventListener("click", () => control("start"));
byId("stop").addEventListener("click", () => control("stop"));
byId("refresh-logs").addEventListener("click", loadLogs);
byId("older").addEventListener("click", () => loadHistory(
  history.page.next_before, [...history.previous, history.before],
));
byId("newer").addEventListener("click", () => {
  const previous = history.previous.slice();
  loadHistory(previous.pop(), previous);
});
byId("latest").addEventListener("click", () => loadHistory());
window.addEventListener("resize", drawCharts);
renderHistory();
loadHistory();
loadStatus();
loadLogs();
window.setInterval(() => {
  if (history.before === null) loadHistory();
  loadStatus();
}, 5000);
