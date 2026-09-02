"use strict";

const HOST = "app.zen_tab_launcher";
let port = null;
let publishTimer = null;
let reconnectTimer = null;
let lastSnapshot = null;
const tabCache = new Map();

function connect() {
  if (port) return;
  try {
    port = browser.runtime.connectNative(HOST);
    port.onMessage.addListener(handleMessage);
    port.onDisconnect.addListener(() => {
      port = null;
      scheduleReconnect();
    });
    publishTabs();
  } catch {
    port = null;
    scheduleReconnect();
  }
}

function scheduleReconnect() {
  if (reconnectTimer) return;
  reconnectTimer = setTimeout(() => {
    reconnectTimer = null;
    connect();
  }, 2000);
}

function schedulePublish() {
  if (publishTimer) clearTimeout(publishTimer);
  publishTimer = setTimeout(() => {
    publishTimer = null;
    publishTabs();
  }, 500);
}

async function publishTabs() {
  if (!port) return;
  const tabs = await browser.tabs.query({});
  const activeIds = new Set(tabs.map(tab => tab.id));
  for (const tabId of tabCache.keys()) {
    if (!activeIds.has(tabId)) tabCache.delete(tabId);
  }
  const visible = tabs
    .filter(tab => /^https?:\/\//.test(tab.url || ""))
    .map(tab => {
      const cached = tabCache.get(tab.id);
      if (cached?.url === tab.url) return cached;
      const snapshot = {
        id: String(tab.id),
        windowId: tab.windowId,
        title: tab.title || tab.url,
        url: tab.url,
        favIconUrl: tab.favIconUrl || null
      };
      tabCache.set(tab.id, snapshot);
      return snapshot;
    })
    .sort((left, right) => Number(left.id) - Number(right.id));
  const snapshot = JSON.stringify(visible);
  if (snapshot === lastSnapshot) return;
  lastSnapshot = snapshot;
  port.postMessage({ type: "snapshot", tabs: visible });
}

async function handleMessage(message) {
  if (message.type !== "request") return;
  try {
    if (message.action === "focus-tab") {
      await focusTab(Number(message.tabId));
    } else if (message.action === "open-target") {
      await openTarget(message);
    } else {
      throw new Error(`Unsupported action: ${message.action}`);
    }
    port?.postMessage({ type: "response", requestId: message.requestId, ok: true });
  } catch (error) {
    port?.postMessage({
      type: "response",
      requestId: message.requestId,
      ok: false,
      error: error instanceof Error ? error.message : String(error)
    });
  }
}

async function focusTab(tabId) {
  if (!Number.isInteger(tabId)) throw new Error("Invalid tab identifier");
  const tab = await browser.tabs.get(tabId);
  await browser.tabs.update(tabId, { active: true });
  await browser.windows.update(tab.windowId, { focused: true });
}

async function openTarget(target) {
  const cookieStoreId = await resolveContainer(target.container);
  if (target.open !== "always-new") {
    const tabs = await browser.tabs.query({});
    const match = tabs.find(tab => matchesTarget(tab, target, cookieStoreId));
    if (match) {
      await focusTab(match.id);
      return;
    }
    if (target.open === "focus") throw new Error(`No open tab matches ${target.targetId}`);
  }
  const options = { url: target.url, active: true };
  if (cookieStoreId) options.cookieStoreId = cookieStoreId;
  const tab = await browser.tabs.create(options);
  await browser.windows.update(tab.windowId, { focused: true });
}

function matchesTarget(tab, target, cookieStoreId) {
  if (!tab.url) return false;
  if (cookieStoreId && tab.cookieStoreId !== cookieStoreId) return false;
  if (target.match === "exact") return normalizedUrl(tab.url) === normalizedUrl(target.url);
  return origin(tab.url) === origin(target.url);
}

function normalizedUrl(value) {
  const url = new URL(value);
  url.hash = "";
  return url.href;
}

function origin(value) {
  return new URL(value).origin;
}

async function resolveContainer(name) {
  if (!name) return null;
  const identities = await browser.contextualIdentities.query({});
  const identity = identities.find(candidate => candidate.name.toLowerCase() === name.toLowerCase());
  if (!identity) throw new Error(`Unknown container: ${name}`);
  return identity.cookieStoreId;
}

browser.tabs.onCreated.addListener(schedulePublish);
browser.tabs.onRemoved.addListener(tabId => {
  tabCache.delete(tabId);
  schedulePublish();
});
browser.tabs.onUpdated.addListener((tabId, changeInfo) => {
  if (!("url" in changeInfo)) return;
  tabCache.delete(tabId);
  schedulePublish();
});

connect();
