"use strict";
// Kohaku Studio フロントエンド。外部ライブラリ非依存。
// UIはAPIにCommandを投げるだけで、データ処理はすべてRust側で行う。

const $ = (id) => document.getElementById(id);

let datasets = [];
let charts = [];
let currentDataset = null;
let lastSqlResult = null;
let editingChartId = null;

// ---------- 共通 ----------

async function api(path, body) {
  const r = await fetch(path, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body || {}),
  });
  const j = await r.json();
  if (!j.ok) throw new Error(j.error || "APIエラー");
  return j.data;
}

function setStatus(msg, isError) {
  const el = $("status");
  el.textContent = msg;
  el.className = isError ? "error" : "";
}

function esc(s) {
  return String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

function qi(name) {
  // SQLite識別子クオート
  return '"' + String(name).replace(/"/g, '""') + '"';
}

function fmtCell(v) {
  if (v === null || v === undefined) return { text: "null", cls: "null" };
  if (typeof v === "number") {
    const text = Number.isInteger(v) ? v.toLocaleString() : String(Math.round(v * 10000) / 10000);
    return { text, cls: "num" };
  }
  return { text: String(v), cls: "" };
}

function renderTable(container, columns, rows, maxRows) {
  maxRows = maxRows || 500;
  const shown = rows.slice(0, maxRows);
  let html = '<table class="grid"><thead><tr>';
  for (const c of columns) html += `<th>${esc(typeof c === "string" ? c : c.name + " (" + c.type + ")")}</th>`;
  html += "</tr></thead><tbody>";
  for (const row of shown) {
    html += "<tr>";
    for (const v of row) {
      const f = fmtCell(v);
      html += `<td class="${f.cls}">${esc(f.text)}</td>`;
    }
    html += "</tr>";
  }
  html += "</tbody></table>";
  if (rows.length > maxRows) html += `<div class="hint">${rows.length.toLocaleString()}行中 先頭${maxRows}行を表示</div>`;
  container.innerHTML = html;
}

function switchTab(name) {
  document.querySelectorAll(".tab").forEach((t) => t.classList.toggle("active", t.dataset.tab === name));
  document.querySelectorAll(".pane").forEach((p) => p.classList.toggle("active", p.id === "tab-" + name));
  if (name === "dashboard") renderDashboard();
  if (name === "analytics") {
    renderAnDatasetSelect();
    if (!anColumns.length) anLoadColumns();
  }
}

// ---------- データセット ----------

async function refreshState() {
  const st = await api("/api/state");
  datasets = st.datasets || [];
  charts = st.charts || [];
  $("project-name").textContent = st.project_name || "無題プロジェクト";
  renderSidebar();
  renderChartList();
  renderDatasetSelect();
  renderAnDatasetSelect();
  renderHistory(st.queries || []);
}

function renderSidebar() {
  const ul = $("dataset-list");
  ul.innerHTML = "";
  for (const d of datasets) {
    const li = document.createElement("li");
    li.classList.toggle("active", d.name === currentDataset);
    li.innerHTML = `<span class="ds-name" title="${esc(d.path)}">${esc(d.name)}</span>
      <span class="ds-rows">${(d.row_count || 0).toLocaleString()}行</span>
      <button class="ds-del" title="削除">✕</button>`;
    li.querySelector(".ds-name").onclick = () => showDataset(d.name);
    li.querySelector(".ds-del").onclick = async (e) => {
      e.stopPropagation();
      if (!confirm(`データセット「${d.name}」を削除しますか?`)) return;
      try {
        await api("/api/dataset/delete", { name: d.name });
        if (currentDataset === d.name) currentDataset = null;
        await refreshState();
        setStatus(`削除しました: ${d.name}`);
      } catch (err) {
        setStatus(err.message, true);
      }
    };
    ul.appendChild(li);
  }
}

async function showDataset(name) {
  currentDataset = name;
  renderSidebar();
  switchTab("data");
  const d = datasets.find((x) => x.name === name);
  try {
    const r = await api("/api/query", { sql: `SELECT * FROM ${qi(name)} LIMIT 200`, limit: 200 });
    const schemaText = (d && d.schema)
      ? d.schema.columns.map((c) => `${c.name}:${c.data_type === "Int64" ? "整数" : c.data_type === "Float64" ? "実数" : c.data_type === "Utf8" ? "文字列" : c.data_type === "Boolean" ? "真偽" : c.data_type}`).join("、 ")
      : "";
    $("data-info").innerHTML = `<b>${esc(name)}</b> — ${(d ? d.row_count : 0).toLocaleString()}行<br><span class="hint">${esc(schemaText)}</span>`;
    renderTable($("data-table"), r.columns, r.rows, 200);
  } catch (err) {
    setStatus(err.message, true);
  }
}

// ---------- インポートウィザード ----------

const imp = { dir: "", path: "", connector: "", objects: [] };

function openImport() {
  $("import-modal").classList.remove("hidden");
  $("imp-config").classList.add("hidden");
  $("imp-msg").textContent = "";
  $("imp-preview").innerHTML = "";
  browse(localStorage.getItem("kohaku.lastDir") || "");
}

async function browse(path) {
  try {
    const r = await api("/api/browse", { path });
    imp.dir = r.path;
    localStorage.setItem("kohaku.lastDir", r.path);
    $("imp-path").value = r.path;
    const ul = $("imp-entries");
    ul.innerHTML = "";
    if (r.parent) {
      const li = document.createElement("li");
      li.textContent = "📁 ..";
      li.onclick = () => browse(r.parent);
      ul.appendChild(li);
    }
    for (const d of r.dirs) {
      const li = document.createElement("li");
      li.textContent = "📁 " + d;
      li.onclick = () => browse(r.path + "\\" + d);
      ul.appendChild(li);
    }
    for (const f of r.files) {
      const li = document.createElement("li");
      const kb = Math.max(1, Math.round(f.size / 1024));
      li.innerHTML = `<span>📄 ${esc(f.name)}</span><span class="fsize">${kb.toLocaleString()} KB</span>`;
      li.onclick = () => {
        ul.querySelectorAll("li").forEach((x) => x.classList.remove("sel"));
        li.classList.add("sel");
        selectFile(f.name);
      };
      ul.appendChild(li);
    }
  } catch (err) {
    setStatus(err.message, true);
  }
}

async function selectFile(name) {
  imp.path = imp.dir.replace(/[\\/]$/, "") + "\\" + name;
  try {
    const r = await api("/api/objects", { path: imp.path });
    imp.connector = r.connector;
    imp.objects = r.objects;
    const sel = $("imp-object");
    sel.innerHTML = "";
    for (const o of r.objects) {
      const op = document.createElement("option");
      op.value = o;
      op.textContent = o;
      sel.appendChild(op);
    }
    $("imp-delim-row").classList.toggle("hidden", r.connector !== "csv");
    const stem = name.replace(/\.[^.]+$/, "");
    $("imp-name").value = sanitizeName(r.connector === "excel" && r.objects.length > 1 ? stem + "_" + r.objects[0] : stem);
    $("imp-config").classList.remove("hidden");
    await refreshImportPreview();
  } catch (err) {
    setStatus(err.message, true);
  }
}

function importOptions() {
  const delim = $("imp-delim").value;
  return {
    header_row: parseInt($("imp-header").value, 10) || 0,
    skip_rows: 0,
    delimiter: delim === "" ? null : delim,
  };
}

function sanitizeName(s) {
  return s.replace(/[^\w-￿]/g, "_");
}

async function refreshImportPreview() {
  if (!imp.path) return;
  $("imp-msg").textContent = "プレビュー読み込み中...";
  try {
    const r = await api("/api/preview", {
      path: imp.path,
      object: $("imp-object").value,
      options: Object.assign(importOptions(), { max_rows: 50 }),
    });
    renderTable($("imp-preview"), r.columns, r.rows, 50);
    $("imp-msg").textContent = "";
  } catch (err) {
    $("imp-msg").textContent = err.message;
  }
}

async function doImport() {
  if (!imp.path) return;
  $("imp-msg").textContent = "取り込み中...";
  try {
    const r = await api("/api/import", {
      path: imp.path,
      object: $("imp-object").value,
      name: $("imp-name").value,
      options: importOptions(),
    });
    $("import-modal").classList.add("hidden");
    await refreshState();
    setStatus(`インポート完了: ${r.name} (${r.rows.toLocaleString()}行)`);
    showDataset(r.name);
  } catch (err) {
    $("imp-msg").textContent = err.message;
  }
}

// ---------- SQL ----------

async function runSql() {
  const sql = $("sql-input").value.trim();
  if (!sql) return;
  $("sql-msg").textContent = "実行中...";
  $("sql-msg").className = "hint";
  const t0 = performance.now();
  try {
    const r = await api("/api/query", { sql, limit: 2000 });
    lastSqlResult = r;
    const ms = Math.round(performance.now() - t0);
    $("sql-msg").textContent = `${r.total_returned.toLocaleString()}行${r.truncated ? "(2000行で打ち切り)" : ""} — ${ms}ms`;
    renderTable($("sql-table"), r.columns, r.rows, 500);
    const st = await api("/api/state");
    renderHistory(st.queries || []);
  } catch (err) {
    $("sql-msg").textContent = err.message;
    $("sql-msg").className = "hint error";
  }
}

function renderHistory(queries) {
  const sel = $("sql-history");
  sel.innerHTML = '<option value="">履歴...</option>';
  for (const q of queries) {
    const op = document.createElement("option");
    op.value = q;
    op.textContent = q.length > 60 ? q.slice(0, 60) + "…" : q;
    sel.appendChild(op);
  }
}

function exportCsv() {
  if (!lastSqlResult) return;
  const { columns, rows } = lastSqlResult;
  const escape = (v) => {
    if (v === null || v === undefined) return "";
    const s = String(v);
    return /[",\n]/.test(s) ? '"' + s.replace(/"/g, '""') + '"' : s;
  };
  let csv = columns.map(escape).join(",") + "\n";
  for (const r of rows) csv += r.map(escape).join(",") + "\n";
  const blob = new Blob(["﻿" + csv], { type: "text/csv" });
  const a = document.createElement("a");
  a.href = URL.createObjectURL(blob);
  a.download = "query_result.csv";
  a.click();
  URL.revokeObjectURL(a.href);
}

// ---------- チャート ----------

function chartSpecFromForm() {
  return {
    id: editingChartId || Date.now(),
    name: $("ch-name").value.trim() || "チャート",
    chart_type: $("ch-type").value,
    source: $("ch-source-kind").value === "dataset"
      ? { kind: "dataset", dataset: $("ch-dataset").value }
      : { kind: "sql", sql: $("ch-sql").value.trim() },
    x: $("ch-x").value,
    y: $("ch-y").value,
    agg: $("ch-agg").value,
    bins: parseInt($("ch-bins").value, 10) || 20,
  };
}

function loadChartToForm(spec) {
  editingChartId = spec.id;
  $("ch-name").value = spec.name || "";
  $("ch-type").value = spec.chart_type || "bar";
  $("ch-source-kind").value = spec.source && spec.source.kind === "sql" ? "sql" : "dataset";
  if (spec.source && spec.source.kind === "sql") $("ch-sql").value = spec.source.sql || "";
  updateChartFormVisibility();
  renderDatasetSelect();
  if (spec.source && spec.source.kind === "dataset") $("ch-dataset").value = spec.source.dataset || "";
  loadChartColumns().then(() => {
    $("ch-x").value = spec.x || "";
    $("ch-y").value = spec.y || "";
    $("ch-agg").value = spec.agg || "none";
    $("ch-bins").value = spec.bins || 20;
    previewChart();
  });
  renderChartList();
}

function updateChartFormVisibility() {
  const kind = $("ch-source-kind").value;
  const type = $("ch-type").value;
  $("ch-dataset-row").classList.toggle("hidden", kind !== "dataset");
  $("ch-sql-row").classList.toggle("hidden", kind !== "sql");
  $("ch-bins-row").classList.toggle("hidden", type !== "histogram");
  $("ch-y-row").classList.toggle("hidden", type === "histogram" || type === "table");
  $("ch-x-row").classList.toggle("hidden", type === "table");
  $("ch-agg-row").classList.toggle("hidden", type === "histogram" || type === "table" || type === "scatter");
}

function renderDatasetSelect() {
  const sel = $("ch-dataset");
  const cur = sel.value;
  sel.innerHTML = "";
  for (const d of datasets) {
    const op = document.createElement("option");
    op.value = d.name;
    op.textContent = d.name;
    sel.appendChild(op);
  }
  if (cur && datasets.some((d) => d.name === cur)) sel.value = cur;
}

function stripSemi(sql) {
  return sql.trim().replace(/;+\s*$/, "");
}

function chartBaseSql(spec) {
  return spec.source.kind === "dataset"
    ? `SELECT * FROM ${qi(spec.source.dataset)}`
    : stripSemi(spec.source.sql);
}

async function loadChartColumns() {
  const kind = $("ch-source-kind").value;
  let cols = [];
  try {
    if (kind === "dataset") {
      const d = datasets.find((x) => x.name === $("ch-dataset").value);
      if (d && d.schema) cols = d.schema.columns.map((c) => c.name);
    } else {
      const sql = stripSemi($("ch-sql").value);
      if (sql) {
        const r = await api("/api/query", { sql: `SELECT * FROM (${sql}) LIMIT 1`, limit: 1 });
        cols = r.columns;
      }
    }
  } catch (e) {
    /* SQL未完成時は無視 */
  }
  for (const id of ["ch-x", "ch-y"]) {
    const sel = $(id);
    const cur = sel.value;
    sel.innerHTML = "";
    for (const c of cols) {
      const op = document.createElement("option");
      op.value = c;
      op.textContent = c;
      sel.appendChild(op);
    }
    if (cols.includes(cur)) sel.value = cur;
  }
}

function buildChartQuery(spec) {
  const base = chartBaseSql(spec);
  const x = qi(spec.x), y = qi(spec.y);
  switch (spec.chart_type) {
    case "table":
      return `SELECT * FROM (${base}) LIMIT 500`;
    case "histogram":
      return `SELECT ${x} AS x FROM (${base}) WHERE ${x} IS NOT NULL LIMIT 100000`;
    case "scatter":
      return `SELECT ${x} AS x, ${y} AS y FROM (${base}) WHERE ${x} IS NOT NULL AND ${y} IS NOT NULL LIMIT 20000`;
    default: {
      if (spec.agg === "none") {
        return `SELECT ${x} AS x, ${y} AS y FROM (${base}) LIMIT 20000`;
      }
      const agg = spec.agg === "count" ? "COUNT(*)" : `${spec.agg.toUpperCase()}(${y})`;
      return `SELECT ${x} AS x, ${agg} AS y FROM (${base}) GROUP BY ${x} ORDER BY ${x} LIMIT 2000`;
    }
  }
}

async function previewChart() {
  const spec = chartSpecFromForm();
  $("chart-title").textContent = spec.name;
  $("chart-msg").textContent = "";
  try {
    const sql = buildChartQuery(spec);
    const r = await api("/api/query", { sql, limit: 100000 });
    drawChartInto($("chart-canvas"), $("chart-table"), spec, r);
  } catch (err) {
    $("chart-msg").textContent = err.message;
  }
}

function drawChartInto(canvas, tableDiv, spec, result) {
  if (spec.chart_type === "table") {
    canvas.classList.add("hidden");
    tableDiv.classList.remove("hidden");
    renderTable(tableDiv, result.columns, result.rows, 500);
  } else {
    canvas.classList.remove("hidden");
    tableDiv.classList.add("hidden");
    renderChart(canvas, spec, result);
  }
}

async function saveChart() {
  const spec = chartSpecFromForm();
  const idx = charts.findIndex((c) => c.id === spec.id);
  if (idx >= 0) charts[idx] = spec;
  else charts.push(spec);
  editingChartId = spec.id;
  try {
    await api("/api/charts/set", { charts });
    renderChartList();
    setStatus(`チャートを保存しました: ${spec.name}`);
  } catch (err) {
    setStatus(err.message, true);
  }
}

function renderChartList() {
  const ul = $("chart-list");
  ul.innerHTML = "";
  for (const c of charts) {
    const li = document.createElement("li");
    li.classList.toggle("active", c.id === editingChartId);
    li.innerHTML = `<span>${esc(c.name)}</span><button class="ch-del" title="削除">✕</button>`;
    li.querySelector("span").onclick = () => loadChartToForm(c);
    li.querySelector(".ch-del").onclick = async (e) => {
      e.stopPropagation();
      charts = charts.filter((x) => x.id !== c.id);
      if (editingChartId === c.id) editingChartId = null;
      await api("/api/charts/set", { charts });
      renderChartList();
    };
    ul.appendChild(li);
  }
}

// ---------- Canvasチャートレンダラ ----------

const CHART_COLORS = { accent: "#4f8ef7", accent2: "#58c9a4", text: "#aab2bf", grid: "#333a46" };

function setupCanvas(canvas) {
  const dpr = window.devicePixelRatio || 1;
  const w = canvas.clientWidth || 600;
  const h = canvas.clientHeight || 400;
  canvas.width = Math.round(w * dpr);
  canvas.height = Math.round(h * dpr);
  const ctx = canvas.getContext("2d");
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, w, h);
  ctx.font = "11px 'Yu Gothic UI', sans-serif";
  return { ctx, w, h };
}

function niceTicks(min, max, count) {
  if (!isFinite(min) || !isFinite(max)) return [0, 1];
  if (min === max) { min -= 1; max += 1; }
  const span = max - min;
  const step0 = span / Math.max(1, count);
  const mag = Math.pow(10, Math.floor(Math.log10(step0)));
  let step = mag;
  for (const m of [1, 2, 2.5, 5, 10]) {
    if (mag * m >= step0) { step = mag * m; break; }
  }
  const ticks = [];
  let t = Math.ceil(min / step) * step;
  for (; t <= max + step * 1e-9; t += step) ticks.push(Math.round(t * 1e9) / 1e9);
  return ticks;
}

function fmtTick(v) {
  if (Math.abs(v) >= 1e6) return (v / 1e6) + "M";
  if (Math.abs(v) >= 1e4) return (v / 1e3) + "k";
  return String(Math.round(v * 1000) / 1000);
}

function renderChart(canvas, spec, result) {
  const { ctx, w, h } = setupCanvas(canvas);
  const xi = result.columns.indexOf("x");
  const yi = result.columns.indexOf("y");
  const C = CHART_COLORS;
  const m = { l: 58, r: 16, t: 14, b: 52 };
  const pw = w - m.l - m.r, ph = h - m.t - m.b;

  const drawAxes = (yTicks, yMin, yMax) => {
    ctx.strokeStyle = C.grid;
    ctx.fillStyle = C.text;
    ctx.lineWidth = 1;
    for (const t of yTicks) {
      const yy = m.t + ph - ((t - yMin) / (yMax - yMin)) * ph;
      ctx.beginPath();
      ctx.moveTo(m.l, yy);
      ctx.lineTo(w - m.r, yy);
      ctx.stroke();
      ctx.textAlign = "right";
      ctx.textBaseline = "middle";
      ctx.fillText(fmtTick(t), m.l - 6, yy);
    }
  };

  const noData = () => {
    ctx.fillStyle = C.text;
    ctx.textAlign = "center";
    ctx.fillText("データがありません", w / 2, h / 2);
  };

  if (spec.chart_type === "histogram") {
    const vals = result.rows.map((r) => Number(r[xi])).filter((v) => isFinite(v));
    if (!vals.length) return noData();
    const lo = Math.min(...vals), hi = Math.max(...vals);
    const nb = Math.max(2, Math.min(200, spec.bins || 20));
    const width = (hi - lo) || 1;
    const counts = new Array(nb).fill(0);
    for (const v of vals) {
      let b = Math.floor(((v - lo) / width) * nb);
      if (b >= nb) b = nb - 1;
      counts[b]++;
    }
    const yMax = Math.max(...counts);
    const yTicks = niceTicks(0, yMax, 5);
    drawAxes(yTicks, 0, yTicks[yTicks.length - 1] || 1);
    const yTop = yTicks[yTicks.length - 1] || 1;
    ctx.fillStyle = C.accent;
    for (let b = 0; b < nb; b++) {
      const bx = m.l + (b / nb) * pw;
      const bw = pw / nb - 1;
      const bh = (counts[b] / yTop) * ph;
      ctx.fillRect(bx, m.t + ph - bh, Math.max(1, bw), bh);
    }
    // X軸ラベル
    ctx.fillStyle = C.text;
    ctx.textAlign = "center";
    ctx.textBaseline = "top";
    const xTicks = niceTicks(lo, hi, 6);
    for (const t of xTicks) {
      const xx = m.l + ((t - lo) / width) * pw;
      if (xx >= m.l - 1 && xx <= w - m.r + 1) ctx.fillText(fmtTick(t), xx, m.t + ph + 6);
    }
    ctx.fillText(spec.x, m.l + pw / 2, h - 16);
    return;
  }

  if (xi < 0 || yi < 0 || !result.rows.length) return noData();
  let rows = result.rows.filter((r) => r[yi] !== null);

  if (spec.chart_type === "scatter" || spec.chart_type === "line") {
    const xNumeric = rows.every((r) => typeof r[xi] === "number");
    let pts;
    if (xNumeric) {
      pts = rows.map((r) => [Number(r[xi]), Number(r[yi])]).filter((p) => isFinite(p[0]) && isFinite(p[1]));
      pts.sort((a, b) => a[0] - b[0]);
    } else {
      pts = rows.map((r, i) => [i, Number(r[yi])]).filter((p) => isFinite(p[1]));
    }
    if (!pts.length) return noData();
    const xs = pts.map((p) => p[0]), ys = pts.map((p) => p[1]);
    const xMin = Math.min(...xs), xMax = Math.max(...xs);
    const yTicks = niceTicks(Math.min(0, Math.min(...ys)), Math.max(...ys), 5);
    const yMin = yTicks[0], yMax = yTicks[yTicks.length - 1];
    drawAxes(yTicks, yMin, yMax);
    const px = (x) => m.l + ((x - xMin) / ((xMax - xMin) || 1)) * pw;
    const py = (y) => m.t + ph - ((y - yMin) / ((yMax - yMin) || 1)) * ph;
    if (spec.chart_type === "line") {
      ctx.strokeStyle = C.accent;
      ctx.lineWidth = 1.6;
      ctx.beginPath();
      pts.forEach((p, i) => (i ? ctx.lineTo(px(p[0]), py(p[1])) : ctx.moveTo(px(p[0]), py(p[1]))));
      ctx.stroke();
    }
    ctx.fillStyle = spec.chart_type === "line" ? C.accent : "rgba(79,142,247,0.65)";
    const rad = spec.chart_type === "line" ? (pts.length > 200 ? 0 : 2.5) : Math.max(1.5, 4 - Math.log10(pts.length + 1));
    if (rad > 0) {
      for (const p of pts) {
        ctx.beginPath();
        ctx.arc(px(p[0]), py(p[1]), rad, 0, Math.PI * 2);
        ctx.fill();
      }
    }
    // X軸
    ctx.fillStyle = C.text;
    ctx.textAlign = "center";
    ctx.textBaseline = "top";
    if (xNumeric) {
      for (const t of niceTicks(xMin, xMax, 6)) {
        const xx = px(t);
        if (xx >= m.l - 1 && xx <= w - m.r + 1) ctx.fillText(fmtTick(t), xx, m.t + ph + 6);
      }
    } else {
      const labels = rows.map((r) => String(r[xi]));
      const stepN = Math.ceil(labels.length / 10);
      labels.forEach((lb, i) => {
        if (i % stepN === 0) ctx.fillText(lb.length > 12 ? lb.slice(0, 12) + "…" : lb, px(i), m.t + ph + 6);
      });
    }
    ctx.fillText(spec.x, m.l + pw / 2, h - 16);
    return;
  }

  // 棒グラフ(カテゴリ)
  const MAX_BARS = 60;
  let note = "";
  if (rows.length > MAX_BARS) {
    note = `${rows.length}カテゴリ中 先頭${MAX_BARS}件を表示`;
    rows = rows.slice(0, MAX_BARS);
  }
  const cats = rows.map((r) => String(r[xi]));
  const ys = rows.map((r) => Number(r[yi]));
  if (!ys.length) return noData();
  const yTicks = niceTicks(Math.min(0, Math.min(...ys)), Math.max(0, Math.max(...ys)), 5);
  const yMin = yTicks[0], yMax = yTicks[yTicks.length - 1];
  drawAxes(yTicks, yMin, yMax);
  const py = (y) => m.t + ph - ((y - yMin) / ((yMax - yMin) || 1)) * ph;
  const bw = pw / cats.length;
  ctx.fillStyle = C.accent;
  cats.forEach((_, i) => {
    const v = ys[i];
    const x0 = m.l + i * bw + bw * 0.12;
    const y0 = py(Math.max(0, v));
    const hh = Math.abs(py(v) - py(0));
    ctx.fillRect(x0, v >= 0 ? y0 : py(0), bw * 0.76, Math.max(1, hh));
  });
  // カテゴリラベル
  ctx.fillStyle = C.text;
  const rotate = cats.length > 8 || cats.some((c) => c.length > 6);
  cats.forEach((c, i) => {
    const cx = m.l + i * bw + bw / 2;
    const label = c.length > 14 ? c.slice(0, 14) + "…" : c;
    const stepN = Math.ceil(cats.length / (rotate ? 30 : 15));
    if (i % stepN !== 0) return;
    ctx.save();
    ctx.translate(cx, m.t + ph + 6);
    if (rotate) {
      ctx.rotate(-Math.PI / 5);
      ctx.textAlign = "right";
      ctx.textBaseline = "top";
    } else {
      ctx.textAlign = "center";
      ctx.textBaseline = "top";
    }
    ctx.fillText(label, 0, 0);
    ctx.restore();
  });
  if (note) {
    ctx.textAlign = "right";
    ctx.textBaseline = "top";
    ctx.fillText(note, w - m.r, 2);
  }
}

// ---------- 分析 ----------

const CLUSTER_COLORS = ["#4f8ef7", "#58c9a4", "#e0a15c", "#e06c75", "#b478e0", "#5cd0e0", "#e0d05c", "#8a94e0"];
let anColumns = []; // {name, numeric}
let lastCluster = null;
let lastClusterReq = null;

function anGetSource() {
  return $("an-source-kind").value === "dataset"
    ? { kind: "dataset", dataset: $("an-dataset").value }
    : { kind: "sql", sql: stripSemi($("an-sql").value) };
}

function renderAnDatasetSelect() {
  const sel = $("an-dataset");
  const cur = sel.value;
  sel.innerHTML = "";
  for (const d of datasets) {
    const op = document.createElement("option");
    op.value = d.name;
    op.textContent = d.name;
    sel.appendChild(op);
  }
  if (cur && datasets.some((d) => d.name === cur)) sel.value = cur;
}

// ソースの列一覧(数値判定つき)を取得して各選択UIへ反映
async function anLoadColumns() {
  anColumns = [];
  $("an-cols-msg").textContent = "";
  try {
    const kind = $("an-source-kind").value;
    if (kind === "dataset") {
      const d = datasets.find((x) => x.name === $("an-dataset").value);
      if (d && d.schema) {
        anColumns = d.schema.columns.map((c) => ({
          name: c.name,
          numeric: c.data_type === "Int64" || c.data_type === "Float64",
        }));
      }
    } else {
      const sql = stripSemi($("an-sql").value);
      if (sql) {
        const r = await api("/api/query", { sql: `SELECT * FROM (${sql}) LIMIT 100`, limit: 100 });
        anColumns = r.columns.map((name, i) => {
          let numeric = false;
          for (const row of r.rows) {
            if (row[i] === null) continue;
            if (typeof row[i] === "number") { numeric = true; } else { numeric = false; break; }
          }
          return { name, numeric };
        });
      }
    }
  } catch (e) {
    $("an-cols-msg").textContent = e.message;
  }
  const numCols = anColumns.filter((c) => c.numeric);
  // 回帰: Y select + X checklist
  const regY = $("reg-y");
  regY.innerHTML = "";
  for (const c of numCols) {
    const op = document.createElement("option");
    op.value = c.name;
    op.textContent = c.name;
    regY.appendChild(op);
  }
  const mkChecklist = (container, prefix) => {
    container.innerHTML = "";
    for (const c of numCols) {
      const lb = document.createElement("label");
      lb.innerHTML = `<input type="checkbox" value="${esc(c.name)}"> ${esc(c.name)}`;
      container.appendChild(lb);
    }
  };
  mkChecklist($("reg-x"));
  mkChecklist($("clu-x"));
  if (!numCols.length) $("an-cols-msg").textContent = "数値列がありません(ソースを選択してください)";
}

function checkedValues(container) {
  return [...container.querySelectorAll("input:checked")].map((x) => x.value);
}

function metricHtml(items) {
  return '<div class="metrics">' + items.map(([l, v]) => `<div class="metric"><div class="mv">${esc(String(v))}</div><div class="ml">${esc(l)}</div></div>`).join("") + "</div>";
}

function fmtNum(v, digits) {
  if (v === null || v === undefined || (typeof v === "number" && !isFinite(v))) return "—";
  if (typeof v !== "number") return esc(String(v));
  return Number(v.toFixed(digits === undefined ? 4 : digits)).toLocaleString(undefined, { maximumFractionDigits: digits === undefined ? 4 : digits });
}

// --- プロファイル ---

async function runProfile() {
  const out = $("an-profile-out");
  out.innerHTML = '<div class="hint">分析中...</div>';
  try {
    const r = await api("/api/analyze/profile", { source: anGetSource() });
    let html = `<div class="hint">${r.n_rows.toLocaleString()}行${r.truncated ? "(上限で打ち切り)" : ""}</div>`;
    // 列統計
    html += "<h4>列статистics</h4>";
    html += '<div class="table-wrap"><table class="grid"><thead><tr><th>列</th><th>型</th><th>件数</th><th>NULL</th><th>個別値</th><th>平均</th><th>標準偏差</th><th>最小</th><th>25%</th><th>中央値</th><th>75%</th><th>最大</th></tr></thead><tbody>';
    for (const c of r.columns) {
      const s = c.stats || {};
      html += `<tr><td>${esc(c.name)}</td><td>${c.kind === "numeric" ? "数値" : "文字列"}</td>
        <td class="num">${c.count.toLocaleString()}</td><td class="num">${c.nulls.toLocaleString()}</td>
        <td class="num">${c.distinct.toLocaleString()}${c.distinct_capped ? "+" : ""}</td>
        <td class="num">${fmtNum(s.mean)}</td><td class="num">${fmtNum(s.std)}</td>
        <td class="num">${fmtNum(s.min)}</td><td class="num">${fmtNum(s.q25)}</td>
        <td class="num">${fmtNum(s.median)}</td><td class="num">${fmtNum(s.q75)}</td>
        <td class="num">${fmtNum(s.max)}</td></tr>`;
    }
    html += "</tbody></table></div>";
    // 相関行列
    const corr = r.correlation;
    if (corr.columns.length >= 2) {
      html += "<h4>相関行列(ピアソン)</h4>";
      html += '<div class="table-wrap"><table class="grid corr"><thead><tr><th></th>';
      for (const c of corr.columns) html += `<th>${esc(c)}</th>`;
      html += "</tr></thead><tbody>";
      corr.matrix.forEach((row, i) => {
        html += `<tr><th>${esc(corr.columns[i])}</th>`;
        row.forEach((v, j) => {
          if (i === j) { html += '<td class="diag">1</td>'; return; }
          if (v === null) { html += "<td>—</td>"; return; }
          const alpha = Math.min(0.85, Math.abs(v));
          const bg = v >= 0 ? `rgba(79,142,247,${alpha})` : `rgba(224,108,117,${alpha})`;
          html += `<td style="background:${bg}">${fmtNum(v, 2)}</td>`;
        });
        html += "</tr>";
      });
      html += "</tbody></table></div>";
    }
    // 強相関ペア
    if (r.top_pairs.length) {
      html += "<h4>相関の強いペア</h4><ul>";
      for (const p of r.top_pairs.slice(0, 5)) {
        html += `<li>${esc(p.a)} × ${esc(p.b)} : r = ${fmtNum(p.r, 3)}</li>`;
      }
      html += "</ul>";
    }
    out.innerHTML = html.replace("列статистics", "列統計");
  } catch (e) {
    out.innerHTML = `<div class="hint error">${esc(e.message)}</div>`;
  }
}

// --- 回帰 ---

async function runRegression() {
  const out = $("reg-out");
  const target = $("reg-y").value;
  const features = checkedValues($("reg-x")).filter((f) => f !== target);
  out.innerHTML = '<div class="hint">分析中...</div>';
  $("reg-canvas").classList.add("hidden");
  try {
    const r = await api("/api/analyze/regression", { source: anGetSource(), target, features });
    let html = metricHtml([
      ["決定係数 R²", fmtNum(r.r2, 4)],
      ["自由度調整済み R²", fmtNum(r.adj_r2, 4)],
      ["RMSE", fmtNum(r.rmse, 4)],
      ["サンプル数", r.n.toLocaleString() + (r.dropped ? `(欠損除外 ${r.dropped}`  + ")" : "")],
    ]);
    html += '<div class="table-wrap"><table class="grid"><thead><tr><th>項</th><th>係数</th><th>標準誤差</th><th>t値</th></tr></thead><tbody>';
    r.names.forEach((n, i) => {
      html += `<tr><td>${esc(n)}</td><td class="num">${fmtNum(r.coef[i], 6)}</td>
        <td class="num">${fmtNum(r.stderr[i], 6)}</td><td class="num">${fmtNum(r.tvalues[i], 2)}</td></tr>`;
    });
    html += "</tbody></table></div>";
    // 回帰式
    let eq = `${esc(r.target)} = ${fmtNum(r.coef[0], 4)}`;
    r.names.slice(1).forEach((n, i) => {
      const c = r.coef[i + 1];
      eq += ` ${c >= 0 ? "+" : "−"} ${fmtNum(Math.abs(c), 4)} × ${esc(n)}`;
    });
    html += `<div class="hint">回帰式: ${eq}</div>`;
    out.innerHTML = html;

    const canvas = $("reg-canvas");
    canvas.classList.remove("hidden");
    if (r.single_feature) {
      const pts = r.points.map((p) => ({ x: p[0], y: p[1] }));
      drawAnScatter(canvas, pts, {
        xlab: r.feature, ylab: r.target,
        line: { slope: r.coef[1], intercept: r.coef[0] },
      });
    } else {
      const pts = r.points.map((p) => ({ x: p[0], y: p[1] }));
      drawAnScatter(canvas, pts, { xlab: "実測値", ylab: "予測値", diag: true });
    }
  } catch (e) {
    out.innerHTML = `<div class="hint error">${esc(e.message)}</div>`;
  }
}

// --- クラスタリング ---

async function runCluster(saveAs) {
  const out = $("clu-out");
  const features = checkedValues($("clu-x"));
  const k = parseInt($("clu-k").value, 10) || 3;
  const req = { source: anGetSource(), features, k };
  if (saveAs) req.save_as = saveAs;
  if (!saveAs) out.innerHTML = '<div class="hint">分析中...</div>';
  try {
    const r = await api("/api/analyze/cluster", req);
    lastCluster = r;
    lastClusterReq = { source: req.source, features, k };
    let html = metricHtml([
      ["クラスタ数", r.k],
      ["使用行数", r.n_used.toLocaleString() + (r.dropped ? `(欠損除外 ${r.dropped})` : "")],
      ["慣性(小さいほど凝集)", fmtNum(r.inertia, 1)],
      ["反復回数", r.iterations],
    ]);
    html += '<div class="table-wrap"><table class="grid"><thead><tr><th>クラスタ</th><th>件数</th>';
    for (const f of r.features) html += `<th>${esc(f)} (中心)</th>`;
    html += "</tr></thead><tbody>";
    r.sizes.forEach((sz, c) => {
      html += `<tr><td><span style="color:${CLUSTER_COLORS[c % CLUSTER_COLORS.length]}">■</span> ${c}</td><td class="num">${sz.toLocaleString()}</td>`;
      r.centroids[c].forEach((v) => (html += `<td class="num">${fmtNum(v, 3)}</td>`));
      html += "</tr>";
    });
    html += "</tbody></table></div>";
    if (r.saved) {
      html += `<div class="hint">データセット「${esc(r.saved.name)}」として保存しました(${r.saved.rows.toLocaleString()}行、cluster列付き)</div>`;
      await refreshState();
    }
    out.innerHTML = html;

    // 軸選択を用意して散布図描画
    const axRow = $("clu-axes-row");
    axRow.classList.remove("hidden");
    for (const id of ["clu-ax", "clu-ay"]) {
      const sel = $(id);
      const cur = sel.value;
      sel.innerHTML = "";
      r.features.forEach((f, i) => {
        const op = document.createElement("option");
        op.value = i;
        op.textContent = f;
        sel.appendChild(op);
      });
      if (cur !== "" && cur < r.features.length) sel.value = cur;
    }
    if (r.features.length > 1 && $("clu-ax").value === $("clu-ay").value) $("clu-ay").value = 1;
    drawClusterScatter();
  } catch (e) {
    out.innerHTML = `<div class="hint error">${esc(e.message)}</div>`;
  }
}

function drawClusterScatter() {
  if (!lastCluster) return;
  const xi = parseInt($("clu-ax").value, 10) || 0;
  const yi = parseInt($("clu-ay").value, 10) || 0;
  const canvas = $("clu-canvas");
  canvas.classList.remove("hidden");
  const nf = lastCluster.features.length;
  const pts = lastCluster.points.map((p) => ({ x: p[xi], y: p[yi], c: p[nf] }));
  drawAnScatter(canvas, pts, {
    xlab: lastCluster.features[xi],
    ylab: lastCluster.features[yi],
    colored: true,
  });
}

// 汎用散布図(回帰線・対角線・クラスタ色分け対応)
function drawAnScatter(canvas, pts, opts) {
  const { ctx, w, h } = setupCanvas(canvas);
  const C = CHART_COLORS;
  const m = { l: 60, r: 16, t: 14, b: 44 };
  const pw = w - m.l - m.r, ph = h - m.t - m.b;
  const valid = pts.filter((p) => isFinite(p.x) && isFinite(p.y));
  if (!valid.length) {
    ctx.fillStyle = C.text;
    ctx.textAlign = "center";
    ctx.fillText("データがありません", w / 2, h / 2);
    return;
  }
  let xMin = Math.min(...valid.map((p) => p.x)), xMax = Math.max(...valid.map((p) => p.x));
  let yMin = Math.min(...valid.map((p) => p.y)), yMax = Math.max(...valid.map((p) => p.y));
  if (opts.diag) {
    xMin = yMin = Math.min(xMin, yMin);
    xMax = yMax = Math.max(xMax, yMax);
  }
  const xTicks = niceTicks(xMin, xMax, 6);
  const yTicks = niceTicks(yMin, yMax, 5);
  xMin = Math.min(xMin, xTicks[0]); xMax = Math.max(xMax, xTicks[xTicks.length - 1]);
  yMin = Math.min(yMin, yTicks[0]); yMax = Math.max(yMax, yTicks[yTicks.length - 1]);
  const px = (x) => m.l + ((x - xMin) / ((xMax - xMin) || 1)) * pw;
  const py = (y) => m.t + ph - ((y - yMin) / ((yMax - yMin) || 1)) * ph;

  ctx.strokeStyle = C.grid;
  ctx.fillStyle = C.text;
  ctx.lineWidth = 1;
  for (const t of yTicks) {
    ctx.beginPath(); ctx.moveTo(m.l, py(t)); ctx.lineTo(w - m.r, py(t)); ctx.stroke();
    ctx.textAlign = "right"; ctx.textBaseline = "middle";
    ctx.fillText(fmtTick(t), m.l - 6, py(t));
  }
  ctx.textAlign = "center"; ctx.textBaseline = "top";
  for (const t of xTicks) {
    if (px(t) >= m.l - 1 && px(t) <= w - m.r + 1) ctx.fillText(fmtTick(t), px(t), m.t + ph + 6);
  }
  const rad = Math.max(1.5, 4 - Math.log10(valid.length + 1));
  for (const p of valid) {
    ctx.fillStyle = opts.colored
      ? CLUSTER_COLORS[(p.c || 0) % CLUSTER_COLORS.length] + "b0"
      : "rgba(79,142,247,0.55)";
    ctx.beginPath();
    ctx.arc(px(p.x), py(p.y), rad, 0, Math.PI * 2);
    ctx.fill();
  }
  ctx.strokeStyle = C.accent2;
  ctx.lineWidth = 1.8;
  if (opts.line) {
    ctx.beginPath();
    ctx.moveTo(px(xMin), py(opts.line.intercept + opts.line.slope * xMin));
    ctx.lineTo(px(xMax), py(opts.line.intercept + opts.line.slope * xMax));
    ctx.stroke();
  }
  if (opts.diag) {
    ctx.setLineDash([5, 4]);
    ctx.beginPath();
    ctx.moveTo(px(xMin), py(xMin));
    ctx.lineTo(px(xMax), py(xMax));
    ctx.stroke();
    ctx.setLineDash([]);
  }
  ctx.fillStyle = C.text;
  ctx.textAlign = "center"; ctx.textBaseline = "top";
  ctx.fillText(opts.xlab || "", m.l + pw / 2, h - 14);
  ctx.save();
  ctx.translate(12, m.t + ph / 2);
  ctx.rotate(-Math.PI / 2);
  ctx.textBaseline = "middle";
  ctx.fillText(opts.ylab || "", 0, 0);
  ctx.restore();
}

// ---------- ダッシュボード ----------

async function renderDashboard() {
  const grid = $("dash-grid");
  grid.innerHTML = "";
  if (!charts.length) {
    grid.innerHTML = '<div class="hint">保存済みチャートがありません。「チャート」タブで作成・保存してください。</div>';
    return;
  }
  for (const spec of charts) {
    const card = document.createElement("div");
    card.className = "dash-card";
    card.innerHTML = `<h4>${esc(spec.name)}</h4>`;
    const canvas = document.createElement("canvas");
    const tdiv = document.createElement("div");
    tdiv.className = "table-wrap hidden";
    card.appendChild(canvas);
    card.appendChild(tdiv);
    grid.appendChild(card);
    try {
      const r = await api("/api/query", { sql: buildChartQuery(spec), limit: 100000 });
      drawChartInto(canvas, tdiv, spec, r);
    } catch (err) {
      card.innerHTML += `<div class="hint error">${esc(err.message)}</div>`;
    }
  }
}

// ---------- プロジェクト ----------

let projectMode = "save";

function openProjectModal(mode) {
  projectMode = mode;
  $("prj-title").textContent = mode === "save" ? "プロジェクトを保存" : "プロジェクトを開く";
  $("prj-msg").textContent = "";
  $("prj-path").value = localStorage.getItem("kohaku.lastProject") || "";
  $("project-modal").classList.remove("hidden");
}

async function projectOk() {
  const path = $("prj-path").value.trim();
  if (!path) return;
  $("prj-msg").textContent = "処理中...";
  try {
    if (projectMode === "save") {
      const r = await api("/api/project/save", { path });
      localStorage.setItem("kohaku.lastProject", r.path);
      $("project-modal").classList.add("hidden");
      setStatus(`保存しました: ${r.path}`);
      await refreshState();
    } else {
      const r = await api("/api/project/load", { path });
      localStorage.setItem("kohaku.lastProject", path);
      $("project-modal").classList.add("hidden");
      currentDataset = null;
      await refreshState();
      if (r.errors && r.errors.length) {
        setStatus("一部読み込み失敗: " + r.errors.join(" / "), true);
      } else {
        setStatus(`プロジェクトを開きました: ${r.project_name}`);
      }
    }
  } catch (err) {
    $("prj-msg").textContent = err.message;
  }
}

// ---------- 初期化 ----------

function init() {
  document.querySelectorAll(".tab").forEach((t) => (t.onclick = () => switchTab(t.dataset.tab)));
  document.querySelectorAll(".close").forEach((b) => (b.onclick = () => $(b.dataset.close).classList.add("hidden")));

  $("btn-import").onclick = openImport;
  $("imp-go").onclick = () => browse($("imp-path").value);
  $("imp-path").addEventListener("keydown", (e) => { if (e.key === "Enter") browse($("imp-path").value); });
  $("imp-object").onchange = () => {
    if (imp.connector === "excel" && imp.objects.length > 1) {
      const stem = imp.path.split("\\").pop().replace(/\.[^.]+$/, "");
      $("imp-name").value = sanitizeName(stem + "_" + $("imp-object").value);
    }
    refreshImportPreview();
  };
  $("imp-header").onchange = refreshImportPreview;
  $("imp-delim").onchange = refreshImportPreview;
  $("imp-import").onclick = doImport;

  $("btn-run-sql").onclick = runSql;
  $("sql-input").addEventListener("keydown", (e) => {
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); runSql(); }
  });
  $("sql-history").onchange = () => {
    if ($("sql-history").value) $("sql-input").value = $("sql-history").value;
  };
  $("btn-sql-csv").onclick = exportCsv;
  $("btn-sql-chart").onclick = () => {
    const sql = $("sql-input").value.trim();
    if (!sql) return;
    editingChartId = null;
    $("ch-source-kind").value = "sql";
    $("ch-sql").value = stripSemi(sql);
    updateChartFormVisibility();
    loadChartColumns();
    switchTab("charts");
  };

  $("ch-type").onchange = updateChartFormVisibility;
  $("ch-source-kind").onchange = () => { updateChartFormVisibility(); loadChartColumns(); };
  $("ch-dataset").onchange = loadChartColumns;
  $("ch-sql").onchange = loadChartColumns;
  $("btn-ch-preview").onclick = previewChart;
  $("btn-ch-save").onclick = saveChart;
  $("btn-ch-new").onclick = () => {
    editingChartId = null;
    $("ch-name").value = "";
    renderChartList();
  };

  // 分析タブ
  document.querySelectorAll(".subtab").forEach((t) => {
    t.onclick = () => {
      document.querySelectorAll(".subtab").forEach((x) => x.classList.toggle("active", x === t));
      document.querySelectorAll(".subpane").forEach((p) => p.classList.toggle("active", p.id === t.dataset.sub));
    };
  });
  $("an-source-kind").onchange = () => {
    const isSql = $("an-source-kind").value === "sql";
    $("an-sql").classList.toggle("hidden", !isSql);
    $("an-dataset").classList.toggle("hidden", isSql);
    anLoadColumns();
  };
  $("an-dataset").onchange = anLoadColumns;
  $("an-sql").onchange = anLoadColumns;
  $("btn-an-profile").onclick = runProfile;
  $("btn-an-reg").onclick = runRegression;
  $("btn-an-clu").onclick = () => runCluster();
  $("btn-clu-save").onclick = () => {
    const name = $("clu-save-name").value.trim();
    if (!name) { setStatus("保存名を入力してください", true); return; }
    runCluster(name);
  };
  $("clu-ax").onchange = drawClusterScatter;
  $("clu-ay").onchange = drawClusterScatter;

  $("btn-dash-refresh").onclick = renderDashboard;
  $("btn-project-save").onclick = () => openProjectModal("save");
  $("btn-project-load").onclick = () => openProjectModal("load");
  $("prj-ok").onclick = projectOk;
  $("prj-path").addEventListener("keydown", (e) => { if (e.key === "Enter") projectOk(); });

  // URLハッシュでタブを開く（例: #sql, #dashboard）。ブックマーク／ディープリンク用。
  const openHashTab = () => {
    const h = location.hash.slice(1);
    if (["data", "sql", "charts", "analytics", "dashboard"].includes(h)) switchTab(h);
  };
  window.addEventListener("hashchange", openHashTab);

  updateChartFormVisibility();
  refreshState()
    .then(openHashTab)
    .catch((e) => setStatus(e.message, true));
}

document.addEventListener("DOMContentLoaded", init);
