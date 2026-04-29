const messagesEl = document.getElementById('messages');
const inputEl = document.getElementById('input');
const sendBtnEl = document.getElementById('sendBtn');
const newSessionBtnEl = document.getElementById('newSessionBtn');
const attachBtnEl = document.getElementById('attachBtn');
const chatTitleEl = document.getElementById('chatTitle');
const attachmentsEl = document.getElementById('attachments');
const attachTipEl = document.getElementById('attachTip');
const chatHeaderEl = document.getElementById('chatHeader');
const resizeGripEl = document.getElementById('resizeGrip');
const allowFileEditsEl = document.getElementById('allowFileEdits');
const allowFileEditsLabelEl = document.getElementById('allowFileEditsLabel');

const DEFAULT_UI_TEXTS = Object.freeze({
  chatTitle: '整理助手',
  newSessionButton: '新会话',
  attachButton: '添加文件',
  allowFileEditsLabel: '允许更改文件',
  attachTipDefault: '支持粘贴图片，后续兼容更多文件',
  inputPlaceholder: '输入内容，Ctrl+Enter 发送',
  sendButton: '发送',
  resizeGripTitle: '拖动缩放窗口',
  assistantGreeting: '需要我帮你做什么？',
  attachmentRemoveButton: '移除',
  defaultDirectoryName: '(默认目录)',
  attachTipRuntime: '工作目录: {workingDirectory} | 温度: {temperature}',
  settingsSource: '配置来源: {source}',
  settingsSourceWithWarning: '配置来源: {source}\n警告: {warning}',
  assistantSummaryLabel: '改动摘要',
  claudeNoOutput: '(Claude 无输出)',
  invokeFailedPrefix: '调用失败: ',
  fileReadFailed: '读取失败',
  thinkingPrompt: 'Thinking',
});
const fallbackThinkingIntervalMs = 1200;

let history = [];
let waiting = false;
let thinkingTimer = null;
let thinkingNode = null;
let thinkingToken = 0;
let thinkingIntervalMs = fallbackThinkingIntervalMs;
let pendingAttachments = [];
let currentThinkingPrompt = DEFAULT_UI_TEXTS.thinkingPrompt;
let thinkingDotCount = 1;
let draggingWindow = false;
let resizingWindow = false;
let runtimeSettings = null;
let runtimeUiTexts = { ...DEFAULT_UI_TEXTS };

function getUiText(key, fallback = '') {
  const runtimeText = runtimeUiTexts[key];
  if (typeof runtimeText === 'string' && runtimeText.trim()) {
    return runtimeText;
  }

  const defaultText = DEFAULT_UI_TEXTS[key];
  if (typeof defaultText === 'string' && defaultText.trim()) {
    return defaultText;
  }

  return String(fallback || '');
}

function fillTemplate(template, values = {}) {
  return String(template || '').replace(/\{(\w+)\}/g, (_all, key) => {
    if (!(key in values)) {
      return '';
    }
    return String(values[key] ?? '');
  });
}

function applyUiTexts() {
  if (chatTitleEl) {
    chatTitleEl.textContent = getUiText('chatTitle', '整理助手');
  }
  if (newSessionBtnEl) {
    newSessionBtnEl.textContent = getUiText('newSessionButton', '新会话');
  }
  if (attachBtnEl) {
    attachBtnEl.textContent = getUiText('attachButton', '添加文件');
  }
  if (allowFileEditsLabelEl) {
    allowFileEditsLabelEl.textContent = getUiText('allowFileEditsLabel', '允许更改文件');
  }
  if (inputEl) {
    inputEl.placeholder = getUiText('inputPlaceholder', '输入内容，Ctrl+Enter 发送');
  }
  if (sendBtnEl) {
    sendBtnEl.textContent = getUiText('sendButton', '发送');
  }
  if (resizeGripEl) {
    resizeGripEl.title = getUiText('resizeGripTitle', '拖动缩放窗口');
  }
  if (attachTipEl && !attachTipEl.textContent?.trim()) {
    attachTipEl.textContent = getUiText('attachTipDefault', '支持粘贴图片，后续兼容更多文件');
  }
}

async function ensureRuntimeSettings() {
  try {
    const result = await window.petApi.getSettings();
    if (!result?.ok || !result?.settings) {
      return;
    }

    runtimeSettings = result.settings;
    if (runtimeSettings.uiTexts && typeof runtimeSettings.uiTexts === 'object' && !Array.isArray(runtimeSettings.uiTexts)) {
      runtimeUiTexts = {
        ...DEFAULT_UI_TEXTS,
        ...runtimeSettings.uiTexts,
      };
    }

    applyUiTexts();

    const temp = Number(runtimeSettings.thinkingTemperature);
    const tempText = Number.isFinite(temp) ? temp.toFixed(2) : '--';
    const workingDirectory = String(runtimeSettings.workingDirectory || '').trim() || getUiText('defaultDirectoryName', '(默认目录)');

    if (attachTipEl) {
      attachTipEl.textContent = fillTemplate(getUiText('attachTipRuntime', '工作目录: {workingDirectory} | 温度: {temperature}'), {
        workingDirectory,
        temperature: tempText,
      });
      attachTipEl.title = result.warning
        ? fillTemplate(getUiText('settingsSourceWithWarning', '配置来源: {source}\n警告: {warning}'), {
          source: result.source || 'unknown',
          warning: result.warning,
        })
        : fillTemplate(getUiText('settingsSource', '配置来源: {source}'), {
          source: result.source || 'unknown',
        });
    }
  } catch {
    // Keep defaults when settings load fails.
  }
}

function renderLatexInMessage(element) {
  const autoRender = window.renderMathInElement;
  if (!element || typeof autoRender !== 'function') {
    return;
  }

  try {
    autoRender(element, {
      delimiters: [
        { left: '$$', right: '$$', display: true },
        { left: '\\[', right: '\\]', display: true },
        { left: '\\(', right: '\\)', display: false },
        { left: '$', right: '$', display: false },
      ],
      ignoredTags: ['script', 'noscript', 'style', 'textarea', 'pre', 'code'],
      throwOnError: false,
      strict: 'ignore',
    });
  } catch {
    // Keep plain text when rendering fails.
  }
}

function escapeHtml(text) {
  return String(text || '')
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#39;');
}

function normalizeLatexForRendering(text) {
  let out = String(text || '');

  const unsupportedStructureRe = /\\(begin|end|item|textbf|textit|underline|section|subsection|paragraph|documentclass|usepackage)\b/i;

  const convertNarrativeLatexToMarkdown = (input) => {
    const lines = String(input || '').split(/\r?\n/);
    const converted = [];
    for (const rawLine of lines) {
      let line = rawLine.trim();
      if (!line) {
        converted.push('');
        continue;
      }

      if (/^\\begin\{enumerate\}/i.test(line) || /^\\end\{enumerate\}/i.test(line)) {
        continue;
      }

      line = line.replace(/^\\item\s*/i, '- ');
      line = line.replace(/\\textbf\{([^}]*)\}/g, '**$1**');
      line = line.replace(/\\textit\{([^}]*)\}/g, '*$1*');
      converted.push(line);
    }
    return converted.join('\n').trim();
  };

  // Convert explicit math/latex fenced blocks to KaTeX display delimiters.
  out = out.replace(/```(?:latex|tex|math)\s*\n([\s\S]*?)```/gi, (_all, body) => {
    const block = String(body || '').trim();
    if (unsupportedStructureRe.test(block)) {
      return `\n${convertNarrativeLatexToMarkdown(block)}\n`;
    }
    return `\n$$\n${block}\n$$\n`;
  });

  const hasAnyMathDelimiter = /\$\$|\$[^$\n]+\$|\\\(|\\\)|\\\[|\\\]/.test(out);

  // If model returns bare equation lines without delimiters, wrap those lines.
  if (!hasAnyMathDelimiter) {
    const lines = out.split(/\r?\n/);
    const normalizedLines = lines.map((line) => {
      const t = line.trim();
      if (!t) {
        return line;
      }

      const looksLikeLatex = /\\(frac|sum|int|lim|sqrt|alpha|beta|gamma|theta|pi|cdot|times|left|right|begin|end)/.test(t)
        || (/[=^_{}]/.test(t) && /\\[a-zA-Z]+/.test(t));
      const hasUnsupportedStructure = unsupportedStructureRe.test(t);

      // Avoid wrapping list/code-like or very long narrative lines.
      const looksLikeListOrCode = /^[-*+]|^\d+\.|`/.test(t);
      if (!looksLikeLatex || hasUnsupportedStructure || looksLikeListOrCode || t.length > 220) {
        return line;
      }

      return `$$ ${t} $$`;
    });
    out = normalizedLines.join('\n');
  }

  return out;
}

function renderMarkdownToSafeHtml(markdownText) {
  const raw = normalizeLatexForRendering(markdownText);
  const markedApi = window.marked;
  const purifyApi = window.DOMPurify;

  if (!markedApi || !purifyApi || typeof markedApi.parse !== 'function') {
    return escapeHtml(raw).replace(/\r?\n/g, '<br/>');
  }

  // Keep math delimiters/text intact so Markdown does not consume underscores/backslashes.
  const mathSegments = [];
  const protectedMarkdown = raw.replace(/(\$\$[\s\S]+?\$\$|\\\[[\s\S]+?\\\]|\\\([\s\S]+?\\\)|\$[^$\n]+\$)/g, (full) => {
    const idx = mathSegments.length;
    mathSegments.push(full);
    return `@@MATH_SEGMENT_${idx}@@`;
  });

  const markdownHtml = markedApi.parse(protectedMarkdown, {
    breaks: true,
    gfm: true,
  });

  let safeHtml = purifyApi.sanitize(markdownHtml, {
    ALLOWED_URI_REGEXP: /^(?:(?:https?|mailto|tel|data):|[^a-z]|[a-z+.-]+(?:[^a-z+.-:]|$))/i,
  });

  safeHtml = safeHtml.replace(/@@MATH_SEGMENT_(\d+)@@/g, (_all, indexText) => {
    const idx = Number(indexText);
    return escapeHtml(mathSegments[idx] || '');
  });

  return safeHtml;
}

function formatSize(bytes) {
  const n = Number(bytes) || 0;
  if (n >= 1024 * 1024) {
    return `${(n / (1024 * 1024)).toFixed(1)}MB`;
  }
  if (n >= 1024) {
    return `${Math.round(n / 1024)}KB`;
  }
  return `${n}B`;
}

function splitAssistantSummary(text) {
  const raw = String(text || '').trim();
  const label = String(getUiText('assistantSummaryLabel', '改动摘要')).replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const match = raw.match(new RegExp(`\\[\\s*${label}\\s*[:：]\\s*([^\\]]*?)\\s*\\]\\s*$`));
  if (!match) {
    return { body: raw, summary: '' };
  }

  const summary = String(match[1] || '').trim();
  const body = raw.slice(0, match.index).trimEnd();
  return { body, summary };
}

function addMessage(role, text) {
  const row = document.createElement('div');
  row.className = `msg-row ${role}`;

  const item = document.createElement('div');
  item.className = `msg ${role}`;

  if (role === 'assistant') {
    const parsed = splitAssistantSummary(text);
    const body = document.createElement('div');
    body.className = 'msg-body';
    body.innerHTML = renderMarkdownToSafeHtml(parsed.body || text);
    item.appendChild(body);

    if (parsed.summary) {
      const meta = document.createElement('div');
      meta.className = 'msg-meta';
      meta.textContent = `[${getUiText('assistantSummaryLabel', '改动摘要')}: ${parsed.summary}]`;
      item.appendChild(meta);
    }
  } else {
    item.textContent = text;
  }

  row.appendChild(item);
  if (role === 'assistant') {
    renderLatexInMessage(item);
  }
  messagesEl.appendChild(row);
  messagesEl.scrollTop = messagesEl.scrollHeight;
  return item;
}

async function ensureThinkingInterval() {
  try {
    const result = await window.petApi.getProgressInterval();
    const ms = Number(result?.ms) || fallbackThinkingIntervalMs;
    thinkingIntervalMs = Math.max(600, ms);
  } catch {
    thinkingIntervalMs = fallbackThinkingIntervalMs;
  }
}

async function fetchThinkingPrompt() {
  try {
    const result = await window.petApi.nextProgressWord();
    const text = String(result?.text || '').trim();
    return text || getUiText('thinkingPrompt', 'Thinking');
  } catch {
    return getUiText('thinkingPrompt', 'Thinking');
  }
}

function startThinking() {
  if (thinkingNode) {
    thinkingNode.remove();
  }

  thinkingToken += 1;
  const localToken = thinkingToken;
  thinkingDotCount = 1;
  currentThinkingPrompt = getUiText('thinkingPrompt', 'Thinking');
  thinkingNode = addMessage('assistant', `${currentThinkingPrompt}.`);

  const updateThinkingText = async () => {
    if (thinkingDotCount === 1 || !currentThinkingPrompt) {
      currentThinkingPrompt = await fetchThinkingPrompt();
    }

    if (thinkingNode && localToken === thinkingToken) {
      thinkingNode.textContent = `${currentThinkingPrompt}${'.'.repeat(thinkingDotCount)}`;
    }

    thinkingDotCount += 1;
    if (thinkingDotCount > 3) {
      thinkingDotCount = 1;
    }
  };

  void updateThinkingText();
  thinkingTimer = setInterval(() => {
    void updateThinkingText();
  }, thinkingIntervalMs);
}

function clearThinkingNode() {
  if (thinkingTimer) {
    clearInterval(thinkingTimer);
    thinkingTimer = null;
  }
  thinkingToken += 1;
  if (thinkingNode) {
    thinkingNode.remove();
    thinkingNode = null;
  }
}

function renderAttachments() {
  attachmentsEl.innerHTML = '';
  for (const file of pendingAttachments) {
    const chip = document.createElement('div');
    chip.className = 'attachment-chip';
    chip.title = file.path;

    const name = document.createElement('span');
    name.textContent = `${file.name} (${formatSize(file.size)})`;

    const removeBtn = document.createElement('button');
    removeBtn.type = 'button';
    removeBtn.textContent = getUiText('attachmentRemoveButton', '移除');
    removeBtn.addEventListener('click', () => {
      pendingAttachments = pendingAttachments.filter((item) => item.path !== file.path);
      renderAttachments();
    });

    chip.appendChild(name);
    chip.appendChild(removeBtn);
    attachmentsEl.appendChild(chip);
  }
}

function addAttachments(files) {
  const byPath = new Map(pendingAttachments.map((item) => [item.path, item]));
  for (const file of Array.isArray(files) ? files : []) {
    const filePath = String(file?.path || '').trim();
    if (!filePath) {
      continue;
    }

    byPath.set(filePath, {
      path: filePath,
      name: String(file?.name || 'unnamed'),
      size: Number(file?.size || 0),
      mimeType: String(file?.mimeType || 'application/octet-stream'),
      source: String(file?.source || 'picked'),
    });
  }

  pendingAttachments = Array.from(byPath.values());
  renderAttachments();
}

function fileToDataUrl(file) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(reader.error || new Error(getUiText('fileReadFailed', '读取失败')));
    reader.onload = () => resolve(String(reader.result || ''));
    reader.readAsDataURL(file);
  });
}

function arrayBufferToBase64(buffer) {
  let binary = '';
  const bytes = new Uint8Array(buffer);
  const chunkSize = 0x8000;
  for (let i = 0; i < bytes.length; i += chunkSize) {
    const chunk = bytes.subarray(i, i + chunkSize);
    binary += String.fromCharCode(...chunk);
  }
  return btoa(binary);
}

async function sendMessage() {
  if (waiting) {
    return;
  }

  const text = inputEl.value.trim();
  if (!text) {
    inputEl.focus();
    return;
  }

  waiting = true;
  sendBtnEl.disabled = true;

  history.push({ role: 'user', text });
  addMessage('user', text);
  inputEl.value = '';

  startThinking();

  try {
    const result = await window.petApi.sendMessage({
      text,
      context: history,
      attachments: pendingAttachments,
      allowFileEdits: Boolean(allowFileEditsEl?.checked),
    });

    clearThinkingNode();

    const answer = result?.text || getUiText('claudeNoOutput', '(Claude 无输出)');
    history.push({ role: 'assistant', text: answer });
    addMessage('assistant', answer);
    pendingAttachments = [];
    renderAttachments();
  } catch (error) {
    clearThinkingNode();
    const msg = `${getUiText('invokeFailedPrefix', '调用失败: ')}${error?.message || String(error)}`;
    history.push({ role: 'assistant', text: msg });
    addMessage('assistant', msg);
  } finally {
    waiting = false;
    sendBtnEl.disabled = false;
    inputEl.focus();
  }
}

sendBtnEl.addEventListener('click', sendMessage);

attachBtnEl.addEventListener('click', async () => {
  try {
    const result = await window.petApi.pickFiles();
    addAttachments(result?.files || []);
  } catch {
    // noop
  }
});

inputEl.addEventListener('keydown', (event) => {
  if (event.key === 'Enter' && event.ctrlKey) {
    event.preventDefault();
    sendMessage();
  }
});

inputEl.addEventListener('paste', async (event) => {
  const fileItems = Array.from(event.clipboardData?.items || []).filter((item) => item.kind === 'file');
  if (fileItems.length === 0) {
    return;
  }

  event.preventDefault();
  const added = [];

  for (const item of fileItems) {
    const file = item.getAsFile();
    if (!file) {
      continue;
    }

    try {
      if (String(file.type || '').startsWith('image/')) {
        const dataUrl = await fileToDataUrl(file);
        const saved = await window.petApi.savePastedDataUrl({
          dataUrl,
          fileName: file.name || `pasted-${Date.now()}.png`,
        });
        if (saved?.ok && saved.file) {
          added.push(saved.file);
        }
      } else {
        const base64 = arrayBufferToBase64(await file.arrayBuffer());
        const saved = await window.petApi.savePastedBlob({
          base64,
          fileName: file.name || `pasted-${Date.now()}`,
          mimeType: file.type || 'application/octet-stream',
        });
        if (saved?.ok && saved.file) {
          added.push(saved.file);
        }
      }
    } catch {
      // noop
    }
  }

  if (added.length > 0) {
    addAttachments(added);
  }
});

newSessionBtnEl.addEventListener('click', () => {
  clearThinkingNode();
  waiting = false;
  history = [];
  pendingAttachments = [];
  messagesEl.innerHTML = '';
  renderAttachments();
});

chatHeaderEl?.addEventListener('mousedown', (event) => {
  if (event.button !== 0) {
    return;
  }

  draggingWindow = true;
  window.petApi.dragWindow({
    type: 'start',
    screenX: event.screenX,
    screenY: event.screenY,
  });
});

resizeGripEl?.addEventListener('mousedown', (event) => {
  if (event.button !== 0) {
    return;
  }

  event.preventDefault();
  resizingWindow = true;
  window.petApi.resizeWindow({
    type: 'start',
    screenX: event.screenX,
    screenY: event.screenY,
  });
});

window.addEventListener('mousemove', (event) => {
  if (draggingWindow) {
    window.petApi.dragWindow({
      type: 'move',
      screenX: event.screenX,
      screenY: event.screenY,
    });
  }

  if (resizingWindow) {
    window.petApi.resizeWindow({
      type: 'move',
      screenX: event.screenX,
      screenY: event.screenY,
    });
  }
});

window.addEventListener('mouseup', () => {
  if (draggingWindow) {
    draggingWindow = false;
    window.petApi.dragWindow({ type: 'end' });
  }

  if (resizingWindow) {
    resizingWindow = false;
    window.petApi.resizeWindow({ type: 'end' });
  }
});

applyUiTexts();
renderAttachments();
void (async () => {
  await ensureRuntimeSettings();
  if (messagesEl.childElementCount === 0) {
    addMessage('assistant', getUiText('assistantGreeting', '需要我帮你做什么？'));
  }
})();
void ensureThinkingInterval();
inputEl.focus();
