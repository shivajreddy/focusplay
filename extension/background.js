// FocusPlay Chrome Extension - Background Service Worker
// Communicates with FocusPlay native app via Native Messaging

const NATIVE_HOST = "com.focusplay.host";

// State
let nativePort = null;
let knownTabs = new Map(); // tabId -> { id, title, url, audible, muted }
let reconnectTimer = null;

// ============================================================================
// NATIVE MESSAGING
// ============================================================================

function connectToNative() {
  if (nativePort) return;

  try {
    nativePort = chrome.runtime.connectNative(NATIVE_HOST);
    console.log("[FocusPlay] Connected to native host");

    nativePort.onMessage.addListener((msg) => {
      console.log("[FocusPlay] Received from native:", msg);
      handleNativeMessage(msg);
    });

    nativePort.onDisconnect.addListener(() => {
      const err = chrome.runtime.lastError;
      console.log("[FocusPlay] Disconnected from native host", err?.message);
      nativePort = null;

      // Retry connection after delay
      if (!reconnectTimer) {
        reconnectTimer = setTimeout(() => {
          reconnectTimer = null;
          connectToNative();
        }, 5000);
      }
    });

    // Send current tab state on connect
    sendTabUpdate();
  } catch (e) {
    console.error("[FocusPlay] Failed to connect to native host:", e);
  }
}

function sendToNative(msg) {
  if (nativePort) {
    try {
      nativePort.postMessage(msg);
    } catch (e) {
      console.error("[FocusPlay] Failed to send message:", e);
      nativePort = null;
    }
  }
}

function handleNativeMessage(msg) {
  switch (msg.type) {
    case "play_pause":
      togglePlayPause(msg.tab_id);
      break;
    case "next_track":
      sendMediaAction(msg.tab_id, "nexttrack");
      break;
    case "prev_track":
      sendMediaAction(msg.tab_id, "previoustrack");
      break;
    case "get_tabs":
      sendTabUpdate();
      break;
    default:
      console.warn("[FocusPlay] Unknown message type:", msg.type);
  }
}

// ============================================================================
// TAB MONITORING
// ============================================================================

async function scanAudibleTabs() {
  const tabs = await chrome.tabs.query({ audible: true });
  const currentIds = new Set();

  for (const tab of tabs) {
    currentIds.add(tab.id);
    knownTabs.set(tab.id, {
      id: tab.id,
      title: tab.title || "Untitled",
      url: tab.url || "",
      audible: tab.audible,
      muted: tab.mutedInfo?.muted || false,
    });
  }

  // Remove tabs that are no longer audible
  for (const [tabId] of knownTabs) {
    if (!currentIds.has(tabId)) {
      knownTabs.delete(tabId);
    }
  }
}

async function sendTabUpdate() {
  await scanAudibleTabs();

  const tabs = Array.from(knownTabs.values()).map((t) => ({
    id: t.id,
    title: t.title,
    url: t.url,
    audible: t.audible,
    muted: t.muted,
  }));

  sendToNative({
    type: "tabs_update",
    tabs: tabs,
  });
}

// ============================================================================
// MEDIA CONTROL
// ============================================================================

async function togglePlayPause(tabId) {
  try {
    await chrome.scripting.executeScript({
      target: { tabId: tabId },
      func: () => {
        // Find the first playing or paused media element
        const mediaElements = [
          ...document.querySelectorAll("video"),
          ...document.querySelectorAll("audio"),
        ];

        // Prefer video over audio, and playing over paused
        const playing = mediaElements.find((el) => !el.paused);
        const paused = mediaElements.find((el) => el.paused && el.currentTime > 0);
        const target = playing || paused || mediaElements[0];

        if (target) {
          if (target.paused) {
            target.play();
          } else {
            target.pause();
          }
          return true;
        }

        // Fallback: try using the Media Session API (space key simulation)
        return false;
      },
    });
    console.log(`[FocusPlay] Toggled play/pause on tab ${tabId}`);
  } catch (e) {
    console.error(`[FocusPlay] Failed to toggle play/pause on tab ${tabId}:`, e);
  }
}

async function sendMediaAction(tabId, action) {
  try {
    await chrome.scripting.executeScript({
      target: { tabId: tabId },
      func: (action) => {
        // Try to trigger the navigator.mediaSession action handlers
        if (navigator.mediaSession) {
          // Dispatch the action
          const handlers = navigator.mediaSession;
          // We can't directly call the handlers, but we can simulate via events
        }

        // Fallback: find media elements and handle manually
        const mediaElements = [
          ...document.querySelectorAll("video"),
          ...document.querySelectorAll("audio"),
        ];
        const target = mediaElements.find((el) => !el.paused) || mediaElements[0];

        if (target) {
          if (action === "nexttrack") {
            // Skip forward 10 seconds as fallback
            target.currentTime = Math.min(target.duration, target.currentTime + 10);
          } else if (action === "previoustrack") {
            // Skip back 10 seconds as fallback
            target.currentTime = Math.max(0, target.currentTime - 10);
          }
        }
      },
      args: [action],
    });
  } catch (e) {
    console.error(`[FocusPlay] Failed media action ${action} on tab ${tabId}:`, e);
  }
}

// ============================================================================
// EVENT LISTENERS
// ============================================================================

// Tab updated (title change, audio state change)
chrome.tabs.onUpdated.addListener((tabId, changeInfo, tab) => {
  if (changeInfo.audible !== undefined || changeInfo.title !== undefined) {
    sendTabUpdate();
  }
});

// Tab closed
chrome.tabs.onRemoved.addListener((tabId) => {
  if (knownTabs.has(tabId)) {
    knownTabs.delete(tabId);
    sendTabUpdate();
  }
});

// ============================================================================
// STARTUP
// ============================================================================

// Connect on install/startup
chrome.runtime.onInstalled.addListener(() => {
  console.log("[FocusPlay] Extension installed");
  connectToNative();
});

chrome.runtime.onStartup.addListener(() => {
  console.log("[FocusPlay] Extension started");
  connectToNative();
});

// Also try to connect immediately (for reloads during development)
connectToNative();
