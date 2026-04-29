const petImageEl = document.getElementById('petImage');
const closeBtnEl = document.getElementById('closeBtn');
const petDragZoneEl = document.getElementById('petDragZone');
const petStageEl = document.querySelector('.pet-stage');

const DEFAULT_UI_TEXTS = Object.freeze({
  petDragTitle: '拖动桌宠',
  petCloseTitle: '关闭',
});

function getUiText(uiTexts, key, fallback = '') {
  const text = uiTexts && typeof uiTexts === 'object' ? uiTexts[key] : undefined;
  if (typeof text === 'string' && text.trim()) {
    return text;
  }

  const defaultText = DEFAULT_UI_TEXTS[key];
  if (typeof defaultText === 'string' && defaultText.trim()) {
    return defaultText;
  }

  return String(fallback || '');
}

function applyUiTexts(uiTexts) {
  if (petDragZoneEl) {
    petDragZoneEl.title = getUiText(uiTexts, 'petDragTitle', '拖动桌宠');
  }
  if (closeBtnEl) {
    closeBtnEl.title = getUiText(uiTexts, 'petCloseTitle', '关闭');
  }
}

async function ensureRuntimeSettings() {
  try {
    const result = await window.petApi.getSettings();
    if (!result?.ok || !result?.settings) {
      applyUiTexts(DEFAULT_UI_TEXTS);
      return;
    }

    applyUiTexts(result.settings.uiTexts);
  } catch {
    applyUiTexts(DEFAULT_UI_TEXTS);
  }
}

let draggingWindow = false;
let activePointerId = null;
let dragStartScreenX = 0;
let dragStartScreenY = 0;
let suppressImageClick = false;

async function bounceAndToggleChat() {
  if (petImageEl) {
    petImageEl.classList.remove('jump');
    void petImageEl.offsetWidth;
    petImageEl.classList.add('jump');
  }
  await window.petApi.toggleChatWindow();
}

closeBtnEl?.addEventListener('click', () => {
  window.petApi.closeWindow();
});

petImageEl?.addEventListener('click', async () => {
  if (suppressImageClick) {
    suppressImageClick = false;
    return;
  }

  await bounceAndToggleChat();
});

function canStartDrag(event) {
  if (event.button !== 0) {
    return false;
  }

  const target = event.target;
  if (target && target.closest('#closeBtn')) {
    return false;
  }

  return true;
}

function handlePointerDown(event) {
  if (!canStartDrag(event)) {
    return;
  }

  event.preventDefault();
  activePointerId = event.pointerId;
  dragStartScreenX = event.screenX;
  dragStartScreenY = event.screenY;
  draggingWindow = true;
  suppressImageClick = false;

  const capturer = petStageEl || petDragZoneEl;
  if (capturer && typeof capturer.setPointerCapture === 'function') {
    try {
      capturer.setPointerCapture(event.pointerId);
    } catch {
      // Ignore pointer capture errors and continue with drag.
    }
  }

  window.petApi.dragWindow({
    type: 'start',
    screenX: event.screenX,
    screenY: event.screenY,
  });
}

function handlePointerMove(event) {
  if (!draggingWindow || activePointerId !== event.pointerId) {
    return;
  }

  if (!suppressImageClick) {
    const moved = Math.abs(event.screenX - dragStartScreenX) + Math.abs(event.screenY - dragStartScreenY);
    if (moved > 3) {
      suppressImageClick = true;
    }
  }

  window.petApi.dragWindow({
    type: 'move',
    screenX: event.screenX,
    screenY: event.screenY,
  });
}

function handlePointerEnd(event) {
  if (!draggingWindow || (activePointerId !== null && activePointerId !== event.pointerId)) {
    return;
  }

  const shouldToggle = !suppressImageClick;
  draggingWindow = false;
  activePointerId = null;
  window.petApi.dragWindow({ type: 'end' });

  if (shouldToggle) {
    suppressImageClick = true;
    void bounceAndToggleChat();
  }
}

petDragZoneEl?.addEventListener('pointerdown', handlePointerDown);
petStageEl?.addEventListener('pointerdown', handlePointerDown);
window.addEventListener('pointermove', handlePointerMove);
window.addEventListener('pointerup', handlePointerEnd);
window.addEventListener('pointercancel', handlePointerEnd);

window.addEventListener('blur', () => {
  if (!draggingWindow) {
    return;
  }

  draggingWindow = false;
  activePointerId = null;
  window.petApi.dragWindow({ type: 'end' });
});

void ensureRuntimeSettings();
