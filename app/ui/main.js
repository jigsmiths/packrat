"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const filter = document.getElementById("filter");
const list = document.getElementById("list");
const preview = document.getElementById("preview");
const image = document.getElementById("image");
const meta = document.getElementById("meta");
const hint = document.getElementById("hint");
const count = document.getElementById("count");
const permission = document.getElementById("permission");

const OTHER_KEYS = "⌘P pin • ⌘⌫ delete • Esc close";
const PASTE_HINT = `Enter paste • ⌘↩ copy • ${OTHER_KEYS}`;
const COPY_HINT = `Enter copy • ${OTHER_KEYS}`;
const CONFIRM_HINT = "Delete this pinned clip? ⌘⌫ again to confirm, Esc to cancel";

let listing = null;
let failure = null;
let selectedHash = null;
let pendingDelete = null;
let canPaste = false;
let listRequest = 0;
let previewRequest = 0;
let imageUrl = null;

function rows() {
  return listing ? listing.rows : [];
}

function selectedIndex() {
  return rows().findIndex((row) => row.hash === selectedHash);
}

function selectedRow() {
  return rows()[selectedIndex()] ?? null;
}

async function refresh() {
  const request = ++listRequest;
  let answer = null;
  let error = null;
  try {
    answer = await invoke("list_clips", { query: filter.value });
  } catch (err) {
    error = String(err);
  }
  if (request !== listRequest) return;

  listing = answer;
  failure = error;
  if (selectedIndex() === -1) {
    selectedHash = rows().length > 0 ? rows()[0].hash : null;
  }
  if (pendingDelete !== selectedHash) pendingDelete = null;
  renderList();
  renderStatus();
  renderPreview();
}

function renderList() {
  const items = rows().map((row) => {
    const item = document.createElement("li");
    item.setAttribute("role", "option");
    item.dataset.hash = row.hash;
    const summary = document.createElement("span");
    summary.className = "summary";
    summary.textContent = row.summary === "" ? "(blank)" : row.summary;
    const age = document.createElement("span");
    age.className = "age";
    age.textContent = formatAge(row.created_at_ms);
    age.title = `added ${formatTime(row.created_at_ms)}`;
    item.replaceChildren(summary, age);
    item.classList.toggle("blank", row.summary === "");
    item.classList.toggle("pinned", row.pinned);
    return item;
  });
  if (items.length === 0) {
    const empty = document.createElement("li");
    empty.className = "empty";
    empty.textContent = failure ?? (filter.value ? "No matches" : "Nothing copied yet");
    items.push(empty);
  }
  list.replaceChildren(...items);
  markSelection();
}

const MINUTE_MS = 60 * 1000;
const HOUR_MS = 60 * MINUTE_MS;
const DAY_MS = 24 * HOUR_MS;

function formatAge(timeMs) {
  const elapsed = Math.max(0, Date.now() - timeMs);
  if (elapsed < MINUTE_MS) return "now";
  if (elapsed < HOUR_MS) return `${Math.floor(elapsed / MINUTE_MS)}m`;
  if (elapsed < DAY_MS) return `${Math.floor(elapsed / HOUR_MS)}h`;
  return `${Math.floor(elapsed / DAY_MS)}d`;
}

function formatTime(timeMs) {
  const time = new Date(timeMs);
  const options = { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" };
  if (time.getFullYear() !== new Date().getFullYear()) options.year = "numeric";
  return time.toLocaleString(undefined, options);
}

function markSelection() {
  for (const item of list.children) {
    item.classList.toggle("selected", item.dataset.hash === selectedHash);
  }
  list.querySelector(".selected")?.scrollIntoView({ block: "nearest" });
}

function renderStatus() {
  hint.classList.toggle("warn", pendingDelete !== null);
  if (pendingDelete !== null) {
    hint.textContent = CONFIRM_HINT;
  } else {
    hint.textContent = canPaste ? PASTE_HINT : COPY_HINT;
  }
  permission.hidden = canPaste || pendingDelete !== null;

  if (!listing) {
    count.textContent = "";
  } else {
    const clips = filter.value ? `${listing.matched} of ${listing.total}` : `${listing.total} clips`;
    count.textContent = `${clips} • ${formatBytes(listing.total_size)}`;
  }
}

function formatBytes(bytes) {
  if (bytes < 1024) return `${bytes} bytes`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function showImage(url) {
  if (imageUrl) URL.revokeObjectURL(imageUrl);
  imageUrl = url;
  if (url) {
    image.src = url;
  } else {
    image.removeAttribute("src");
  }
  image.hidden = url === null;
  preview.hidden = url !== null;
}

async function renderPreview() {
  const request = ++previewRequest;
  const row = selectedRow();
  if (!row) {
    preview.textContent = "";
    showImage(null);
    meta.textContent = "";
    return;
  }
  let text = "";
  let url = null;
  let details = "";
  try {
    const parts = [];
    let truncated = false;
    if (row.image) {
      const bytes = await invoke("image_clip", { hash: row.hash });
      url = URL.createObjectURL(new Blob([bytes], { type: "image/png" }));
      parts.push(`PNG, ${formatBytes(row.size)}`);
    } else {
      const shown = await invoke("preview_clip", { hash: row.hash });
      parts.push(`${shown.chars.toLocaleString()} characters`, formatBytes(row.size));
      truncated = shown.truncated;
      text = shown.text;
    }
    if (row.pinned) parts.push("pinned");
    parts.push(`added ${formatTime(row.created_at_ms)}`);
    if (row.last_used_at_ms !== row.created_at_ms) {
      parts.push(`last used ${formatTime(row.last_used_at_ms)}`);
    }
    if (truncated) parts.push("showing the first 100 KB");
    details = parts.join(" • ");
  } catch (err) {
    details = String(err);
  }
  if (request !== previewRequest) {
    if (url) URL.revokeObjectURL(url);
    return;
  }
  preview.textContent = text;
  preview.scrollTop = 0;
  showImage(url);
  meta.textContent = details;
}

function select(hash) {
  if (hash === selectedHash) return;
  selectedHash = hash;
  pendingDelete = null;
  markSelection();
  renderStatus();
  renderPreview();
}

function move(step) {
  const all = rows();
  if (all.length === 0) return;
  const next = Math.min(Math.max(selectedIndex() + step, 0), all.length - 1);
  select(all[next].hash);
}

async function togglePin() {
  const row = selectedRow();
  if (!row) return;
  try {
    await invoke("toggle_pin", { hash: row.hash });
  } catch (err) {
    console.error("toggle_pin", err);
  }
  await refresh();
}

async function copySelected(paste) {
  const row = selectedRow();
  if (!row) return;
  try {
    await invoke("copy_clip", { hash: row.hash, paste });
  } catch (err) {
    console.error("copy_clip", err);
    await refresh();
  }
}

async function checkPermission() {
  try {
    canPaste = await invoke("paste_allowed");
  } catch (err) {
    canPaste = false;
  }
  renderStatus();
}

async function deleteSelected() {
  const row = selectedRow();
  if (!row) return;
  if (row.pinned && pendingDelete !== row.hash) {
    pendingDelete = row.hash;
    renderStatus();
    return;
  }
  const index = selectedIndex();
  const neighbor = rows()[index + 1] ?? rows()[index - 1] ?? null;
  try {
    await invoke("delete_clip", { hash: row.hash });
  } catch (err) {
    console.error("delete_clip", err);
  }
  selectedHash = neighbor ? neighbor.hash : null;
  pendingDelete = null;
  await refresh();
}

async function onEscape() {
  if (pendingDelete !== null) {
    pendingDelete = null;
    renderStatus();
  } else if (filter.value !== "") {
    filter.value = "";
    selectedHash = null;
    await refresh();
  } else {
    await invoke("hide_popup");
  }
}

window.addEventListener("keydown", (event) => {
  const command = event.metaKey && !event.ctrlKey && !event.altKey;
  const control = event.ctrlKey && !event.metaKey && !event.altKey;

  if (event.key === "Escape") {
    onEscape();
  } else if (event.key === "ArrowDown" || (control && event.key === "n")) {
    move(1);
  } else if (event.key === "ArrowUp" || (control && event.key === "p")) {
    move(-1);
  } else if (event.key === "Enter") {
    copySelected(!event.metaKey);
  } else if (command && event.key === "p") {
    togglePin();
  } else if (command && event.key === "Backspace") {
    deleteSelected();
  } else {
    return;
  }
  event.preventDefault();
});

filter.addEventListener("input", () => {
  selectedHash = null;
  refresh();
});

list.addEventListener("click", (event) => {
  const item = event.target.closest("li[data-hash]");
  if (item) select(item.dataset.hash);
  filter.focus();
});

list.addEventListener("dblclick", (event) => {
  if (event.target.closest("li[data-hash]")) copySelected(true);
});

permission.addEventListener("click", () => {
  invoke("open_accessibility_settings");
});

listen("popup-shown", () => {
  filter.value = "";
  selectedHash = null;
  pendingDelete = null;
  filter.focus();
  checkPermission();
  refresh();
});

listen("history-changed", refresh);

checkPermission();
refresh();
