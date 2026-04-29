const { app, BrowserWindow, ipcMain, dialog, screen } = require('electron');
const path = require('path');
const fs = require('fs');
const { spawn } = require('child_process');

const PET_WINDOW_WIDTH = 260;
const PET_WINDOW_HEIGHT = 260;
const CHAT_WINDOW_WIDTH = 500;
const CHAT_WINDOW_HEIGHT = 620;
const CHAT_MIN_WINDOW_WIDTH = 420;
const CHAT_MIN_WINDOW_HEIGHT = 420;
const CHAT_GAP = 2;
const EDGE_PADDING = 8;
const THINKING_SWITCH_INTERVAL_MS = 1200;
const UPLOAD_DIR_NAME = 'desktop-pet-uploads';
const EVERYTHING_AUTO_TRIGGER_ENABLED = true;
const EVERYTHING_SKILL_COMMANDS = new Set(['/everything', '/es']);
const EVERYTHING_SKILL_MAX_RESULTS_IN_PROMPT = 30;
const SORT_SKILL_COMMANDS = new Set(['/sort']);
const SORT_CHANGELOG_FILE_NAME = 'changelog.json';
const SORT_AI_PLAN_MARKER = 'SORT_AI_PLAN_JSON';
const SORT_AI_MAX_PROMPT_ENTRIES = 220;
const DEFAULT_GIT_SNAPSHOT_KEEP_LATEST = 50;
const DEFAULT_SYSTEM_PROMPT = '你是一个AI文件管理与处理助手，语言简洁。如果没有要求优先用中文回复，回答控制在 3-6 句。可以表达轻微情绪和陪伴感，但不要编造事实。';
const DEFAULT_UI_TEXTS = Object.freeze({
  chatTitle: '整理助手',
  newSessionButton: '新会话',
  attachButton: '添加文件',
  allowFileEditsLabel: '允许更改文件',
  attachTipDefault: '支持粘贴图片，后续兼容更多文件',
  inputPlaceholder: '输入内容，Ctrl+Enter 发送',
  sendButton: '发送',
  resizeGripTitle: '拖动缩放窗口',
  petDragTitle: '拖动桌宠',
  petCloseTitle: '关闭',
  assistantGreeting: '需要我帮你做什么？',
  attachmentRemoveButton: '移除',
  defaultDirectoryName: '(默认目录)',
  attachTipRuntime: '工作目录: {workingDirectory} | 温度: {temperature}',
  settingsSource: '配置来源: {source}',
  settingsSourceWithWarning: '配置来源: {source}\n警告: {warning}',
  assistantSummaryLabel: '改动摘要',
  thinkingPrompt: 'Thinking',
  claudeNoOutput: '(Claude 无输出)',
  invokeFailedPrefix: '调用失败: ',
  emptyInputPrompt: '请输入内容。',
  pickFileDialogTitle: '选择文件',
  pickFileFilterSupported: '支持的多模态文件',
  pickFileFilterAll: '所有文件',
  invalidImageData: '无效的图片数据。',
  emptyFileData: '空文件数据。',
});
const FALLBACK_PROGRESS_MESSAGES = [DEFAULT_UI_TEXTS.thinkingPrompt];
const DEFAULT_SETTINGS = Object.freeze({
  workingDirectory: resolveWorkspaceRoot(),
  systemPrompt: DEFAULT_SYSTEM_PROMPT,
  thinkingTemperature: 0.7,
  thinkingIntervalMs: THINKING_SWITCH_INTERVAL_MS,
  gitSnapshotKeepLatest: DEFAULT_GIT_SNAPSHOT_KEEP_LATEST,
  attachmentDirectories: [],
  uiTexts: DEFAULT_UI_TEXTS,
});

function resolveWorkspaceRoot() {
  if (app.isPackaged) {
    return process.resourcesPath;
  }
  return path.resolve(__dirname, '..');
}

function resolveProgressMessagesFilePath() {
  if (app.isPackaged) {
    return path.join(process.resourcesPath, 'progressMessage.txt');
  }
  return path.resolve(__dirname, '..', 'progressMessage.txt');
}

let petWindow = null;
let chatWindow = null;
const dragStateByWebContentsId = new Map();
const resizeStateByWebContentsId = new Map();
let cachedProgressMessages = null;
let runtimeSettings = { ...DEFAULT_SETTINGS };
let runtimeSettingsSource = 'defaults';
let runtimeSettingsWarning = '';
let settingsLoadPromise = null;
const pendingSortPlans = new Map();

function sanitizeUiTexts(rawUiTexts) {
  const merged = { ...DEFAULT_UI_TEXTS };
  const source = rawUiTexts && typeof rawUiTexts === 'object' && !Array.isArray(rawUiTexts)
    ? rawUiTexts
    : {};

  for (const [key, value] of Object.entries(source)) {
    if (typeof value !== 'string') {
      continue;
    }
    const trimmed = value.trim();
    if (trimmed) {
      merged[key] = trimmed;
    }
  }

  return merged;
}

function getUiText(settings, key, fallback = '') {
  const fromSettings = settings?.uiTexts && typeof settings.uiTexts === 'object'
    ? settings.uiTexts[key]
    : undefined;
  if (typeof fromSettings === 'string' && fromSettings.trim()) {
    return fromSettings;
  }

  const fromDefaults = DEFAULT_UI_TEXTS[key];
  if (typeof fromDefaults === 'string' && fromDefaults.trim()) {
    return fromDefaults;
  }

  return String(fallback || '');
}

// On some Windows GPU drivers, transparent windows can render with a black background.
app.disableHardwareAcceleration();

function getWorkAreaForBounds(bounds) {
  const center = {
    x: Math.round(bounds.x + bounds.width / 2),
    y: Math.round(bounds.y + bounds.height / 2),
  };
  return screen.getDisplayNearestPoint(center).workArea;
}

function clampBoundsToArea(bounds, area) {
  const maxX = area.x + area.width - bounds.width;
  const maxY = area.y + area.height - bounds.height;
  return {
    ...bounds,
    x: Math.min(Math.max(bounds.x, area.x), Math.max(area.x, maxX)),
    y: Math.min(Math.max(bounds.y, area.y), Math.max(area.y, maxY)),
  };
}

function getDefaultPetBounds() {
  const area = screen.getPrimaryDisplay().workArea;
  return {
    width: PET_WINDOW_WIDTH,
    height: PET_WINDOW_HEIGHT,
    x: area.x + area.width - PET_WINDOW_WIDTH - EDGE_PADDING,
    y: area.y + area.height - PET_WINDOW_HEIGHT - EDGE_PADDING,
  };
}

function createPetWindow() {
  const bounds = getDefaultPetBounds();
  petWindow = new BrowserWindow({
    width: bounds.width,
    height: bounds.height,
    x: bounds.x,
    y: bounds.y,
    frame: false,
    transparent: true,
    hasShadow: false,
    resizable: false,
    backgroundColor: '#00000000',
    alwaysOnTop: true,
    skipTaskbar: true,
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  });

  petWindow.loadFile(path.join(__dirname, 'renderer', 'pet.html'));
  petWindow.on('closed', () => {
    petWindow = null;
  });
}

function createChatWindow() {
  chatWindow = new BrowserWindow({
    width: CHAT_WINDOW_WIDTH,
    height: CHAT_WINDOW_HEIGHT,
    minWidth: CHAT_MIN_WINDOW_WIDTH,
    minHeight: CHAT_MIN_WINDOW_HEIGHT,
    frame: false,
    transparent: true,
    hasShadow: false,
    backgroundColor: '#00000000',
    alwaysOnTop: true,
    show: false,
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  });

  chatWindow.loadFile(path.join(__dirname, 'renderer', 'chat.html'));
  chatWindow.on('closed', () => {
    chatWindow = null;
  });
}

function positionChatWindow() {
  if (!petWindow || !chatWindow || chatWindow.isDestroyed() || petWindow.isDestroyed()) {
    return;
  }

  const petBounds = petWindow.getBounds();
  const chatBounds = chatWindow.getBounds();
  const area = getWorkAreaForBounds(petBounds);

  let x = petBounds.x - chatBounds.width - CHAT_GAP;
  const yAlignedBottom = petBounds.y + petBounds.height - chatBounds.height;
  let y = yAlignedBottom;

  if (x < area.x + EDGE_PADDING) {
    x = petBounds.x + petBounds.width + CHAT_GAP;
  }

  const maxX = area.x + area.width - chatBounds.width - EDGE_PADDING;
  const minX = area.x + EDGE_PADDING;
  x = Math.min(Math.max(x, minX), Math.max(minX, maxX));

  const maxY = area.y + area.height - chatBounds.height - EDGE_PADDING;
  const minY = area.y + EDGE_PADDING;
  y = Math.min(Math.max(y, minY), Math.max(minY, maxY));

  chatWindow.setPosition(Math.round(x), Math.round(y));
}

function ensurePetWindowInBounds() {
  if (!petWindow || petWindow.isDestroyed()) {
    return;
  }

  const bounds = petWindow.getBounds();
  const area = getWorkAreaForBounds(bounds);
  const next = clampBoundsToArea(bounds, area);
  if (next.x !== bounds.x || next.y !== bounds.y) {
    petWindow.setPosition(next.x, next.y);
  }
}

function toggleChatWindow() {
  if (!chatWindow || chatWindow.isDestroyed()) {
    return false;
  }

  const shouldShow = !chatWindow.isVisible();
  if (shouldShow) {
    positionChatWindow();
    chatWindow.show();
    chatWindow.focus();
  } else {
    chatWindow.hide();
  }
  return shouldShow;
}

function clampRenderText(text) {
  const cleaned = (text || '').replaceAll('\0', ' ');
  const maxChars = 5000;
  if (cleaned.length > maxChars) {
    return `${cleaned.slice(0, maxChars)}\n\n(内容过长，已截断显示)`;
  }
  return cleaned;
}

function loadProgressMessages() {
  if (Array.isArray(cachedProgressMessages) && cachedProgressMessages.length > 0) {
    return cachedProgressMessages;
  }

  const filePath = resolveProgressMessagesFilePath();
  try {
    const raw = fs.readFileSync(filePath, 'utf8');
    const parsed = raw
      .split(/\r?\n/)
      .map((line) => line.trim())
      .map((line) => line.replace(/,$/, '').trim())
      .map((line) => line.replace(/^['\"]|['\"]$/g, '').trim())
      .filter(Boolean);

    if (parsed.length > 0) {
      cachedProgressMessages = parsed;
      return cachedProgressMessages;
    }
  } catch {
    // Keep fallback below.
  }

  cachedProgressMessages = [...FALLBACK_PROGRESS_MESSAGES];
  return cachedProgressMessages;
}

function pickProgressMessage() {
  const messages = loadProgressMessages();
  if (messages.length === 0) {
    return FALLBACK_PROGRESS_MESSAGES[0];
  }

  const index = Math.floor(Math.random() * messages.length);
  return messages[index];
}

function decodeOutputBuffer(buffer) {
  if (!buffer || buffer.length === 0) {
    return '';
  }

  const nulCount = buffer.reduce((sum, byte) => sum + (byte === 0 ? 1 : 0), 0);
  if (nulCount > Math.floor(buffer.length / 6)) {
    return buffer.toString('utf16le').replace(/\u0000/g, '');
  }

  return buffer.toString('utf8');
}

const RUST_SOURCE_MTIME_TTL_MS = 3000;
let rustSourceMtimeCache = {
  workspaceRoot: '',
  checkedAt: 0,
  mtimeMs: 0,
};

function collectLatestMtimeMs(targetPath) {
  let latest = 0;
  const queue = [targetPath];

  while (queue.length > 0) {
    const current = queue.pop();
    if (!current || !fs.existsSync(current)) {
      continue;
    }

    let stat = null;
    try {
      stat = fs.statSync(current);
    } catch {
      continue;
    }

    latest = Math.max(latest, Number(stat.mtimeMs || 0));
    if (!stat.isDirectory()) {
      continue;
    }

    let entries = [];
    try {
      entries = fs.readdirSync(current, { withFileTypes: true });
    } catch {
      continue;
    }

    for (const entry of entries) {
      if (!entry) {
        continue;
      }

      const name = String(entry.name || '').trim();
      if (!name || name === 'target' || name === '.git' || name === 'node_modules') {
        continue;
      }

      if (entry.isDirectory()) {
        queue.push(path.join(current, name));
        continue;
      }

      if (!name.toLowerCase().endsWith('.rs')) {
        continue;
      }

      const filePath = path.join(current, name);
      try {
        const fileStat = fs.statSync(filePath);
        latest = Math.max(latest, Number(fileStat.mtimeMs || 0));
      } catch {
        // Ignore unreadable files and continue scanning.
      }
    }
  }

  return latest;
}

function getLatestRustSourceMtimeMs(workspaceRoot) {
  const now = Date.now();
  if (
    rustSourceMtimeCache.workspaceRoot === workspaceRoot
    && now - rustSourceMtimeCache.checkedAt <= RUST_SOURCE_MTIME_TTL_MS
  ) {
    return rustSourceMtimeCache.mtimeMs;
  }

  const srcDir = path.join(workspaceRoot, 'src');
  let latest = collectLatestMtimeMs(srcDir);

  for (const fileName of ['Cargo.toml', 'Cargo.lock']) {
    const filePath = path.join(workspaceRoot, fileName);
    if (!fs.existsSync(filePath)) {
      continue;
    }

    try {
      const stat = fs.statSync(filePath);
      latest = Math.max(latest, Number(stat.mtimeMs || 0));
    } catch {
      // Ignore and keep latest source mtime.
    }
  }

  rustSourceMtimeCache = {
    workspaceRoot,
    checkedAt: now,
    mtimeMs: latest,
  };

  return latest;
}

function shouldUsePrebuiltRustBinary(executablePath, workspaceRoot) {
  if (!fs.existsSync(executablePath)) {
    return false;
  }

  let binaryMtime = 0;
  try {
    binaryMtime = Number(fs.statSync(executablePath).mtimeMs || 0);
  } catch {
    return false;
  }

  const latestSourceMtime = getLatestRustSourceMtimeMs(workspaceRoot);
  if (!latestSourceMtime) {
    return true;
  }

  return binaryMtime >= latestSourceMtime;
}

function resolveRustCommand(binName) {
  const executableName = process.platform === 'win32' ? `${binName}.exe` : binName;

  if (app.isPackaged) {
    const workspaceRoot = resolveWorkspaceRoot();
    const executablePath = path.join(process.resourcesPath, 'bin', executableName);

    if (!fs.existsSync(executablePath)) {
      throw new Error(`缺少打包的 Rust 可执行文件: ${executablePath}`);
    }

    return {
      command: executablePath,
      args: [],
      workspaceRoot,
      binaryLabel: executablePath,
    };
  }

  const workspaceRoot = resolveWorkspaceRoot();
  const executablePath = path.join(workspaceRoot, 'target', 'debug', executableName);

  if (shouldUsePrebuiltRustBinary(executablePath, workspaceRoot)) {
    return {
      command: executablePath,
      args: [],
      workspaceRoot,
      binaryLabel: executablePath,
    };
  }

  return {
    command: 'cargo',
    args: ['run', '--quiet', '--bin', binName],
    workspaceRoot,
    binaryLabel: `cargo run --bin ${binName}`,
  };
}

function runRustJsonBin(binName, payload = null) {
  const launch = resolveRustCommand(binName);

  return new Promise((resolve, reject) => {
    const child = spawn(launch.command, launch.args, {
      cwd: launch.workspaceRoot,
      stdio: ['pipe', 'pipe', 'pipe'],
      windowsHide: true,
      env: {
        ...process.env,
        PYTHONUTF8: '1',
        PYTHONIOENCODING: 'utf-8',
      },
    });

    const stdoutChunks = [];
    const stderrChunks = [];

    child.stdout.on('data', (chunk) => {
      stdoutChunks.push(Buffer.from(chunk));
    });
    child.stderr.on('data', (chunk) => {
      stderrChunks.push(Buffer.from(chunk));
    });

    child.on('error', (err) => reject(err));
    child.on('close', (code) => {
      const stdout = decodeOutputBuffer(Buffer.concat(stdoutChunks)).trim();
      const stderr = decodeOutputBuffer(Buffer.concat(stderrChunks)).trim();

      if (code !== 0) {
        reject(new Error(stderr || `${launch.binaryLabel} 退出码: ${code}`));
        return;
      }

      try {
        const parsed = JSON.parse(stdout || '{}');
        resolve(parsed);
      } catch (err) {
        reject(new Error(`${launch.binaryLabel} 输出 JSON 解析失败: ${err.message || String(err)}; stdout=${stdout}; stderr=${stderr}`));
      }
    });

    if (child.stdin && !child.stdin.destroyed) {
      if (payload !== null && payload !== undefined) {
        child.stdin.write(JSON.stringify(payload));
      }
      child.stdin.end();
    }
  });
}

function runRustCcConnect(payload) {
  return runRustJsonBin('ccconnect', payload);
}

function runRustEverythingSearch(payload) {
  return runRustJsonBin('cceverything', payload);
}

function runRustSort(payload) {
  return runRustJsonBin('ccsort', payload);
}

function runRustGitConnect(payload) {
  return runRustJsonBin('ccgitconnect', payload);
}

function tokenizeSkillArgs(text) {
  const input = String(text || '').trim();
  if (!input) {
    return [];
  }

  const tokens = [];
  const matcher = /"((?:[^"\\]|\\.)*)"|'((?:[^'\\]|\\.)*)'|(\S+)/g;
  let match = null;
  while ((match = matcher.exec(input)) !== null) {
    const token = match[1] || match[2] || match[3] || '';
    tokens.push(token.replace(/\\(["'\\])/g, '$1'));
  }
  return tokens;
}

function extractEverythingCommandSegment(text) {
  const raw = String(text || '');
  if (!raw.trim()) {
    return '';
  }

  const markerMatch = /(\/everything|\/es)\b/i.exec(raw);
  if (!markerMatch) {
    return '';
  }

  const commandText = markerMatch[1] || '';
  const markerIndex = Number(markerMatch.index) || 0;
  let rest = raw.slice(markerIndex + commandText.length);

  // Allow inputs like "/everything帮我找 main.js" by inserting a space after marker.
  if (rest && !/^\s/.test(rest)) {
    rest = ` ${rest}`;
  }

  return `${commandText}${rest}`.trim();
}

function parseEverythingSkillCommand(text) {
  const originalText = String(text || '').trim();
  const commandSegment = extractEverythingCommandSegment(originalText);
  if (!commandSegment) {
    return null;
  }

  const tokens = tokenizeSkillArgs(commandSegment);
  if (tokens.length === 0) {
    return null;
  }

  const command = String(tokens[0] || '').toLowerCase();
  if (!EVERYTHING_SKILL_COMMANDS.has(command)) {
    return null;
  }

  const request = {
    query: '',
    scope: 'file',
    count: 50,
    offset: 0,
    regex: false,
    wholeWord: false,
    matchCase: false,
  };

  const queryParts = [];
  const errors = [];
  let followUp = '';

  for (let i = 1; i < tokens.length; i += 1) {
    const token = tokens[i];

    if (token === '--ask') {
      followUp = tokens.slice(i + 1).join(' ').trim();
      break;
    }

    if (token === '--scope') {
      const value = String(tokens[i + 1] || '').toLowerCase();
      if (!value) {
        errors.push('参数 --scope 缺少值');
      } else if (value !== 'file' && value !== 'path') {
        errors.push('参数 --scope 仅支持 file 或 path');
      } else {
        request.scope = value;
      }
      i += 1;
      continue;
    }

    if (token === '--count') {
      const value = Number.parseInt(tokens[i + 1], 10);
      if (!Number.isFinite(value)) {
        errors.push('参数 --count 必须是数字');
      } else {
        request.count = Math.min(Math.max(value, 1), 5000);
      }
      i += 1;
      continue;
    }

    if (token === '--offset') {
      const value = Number.parseInt(tokens[i + 1], 10);
      if (!Number.isFinite(value)) {
        errors.push('参数 --offset 必须是数字');
      } else {
        request.offset = Math.max(value, 0);
      }
      i += 1;
      continue;
    }

    if (token === '--host') {
      request.host = String(tokens[i + 1] || '').trim();
      if (!request.host) {
        errors.push('参数 --host 缺少值');
      }
      i += 1;
      continue;
    }

    if (token === '--port') {
      const value = Number.parseInt(tokens[i + 1], 10);
      if (!Number.isFinite(value)) {
        errors.push('参数 --port 必须是数字');
      } else {
        request.port = Math.min(Math.max(value, 1), 65535);
      }
      i += 1;
      continue;
    }

    if (token === '--username') {
      request.username = String(tokens[i + 1] || '').trim();
      if (!request.username) {
        errors.push('参数 --username 缺少值');
      }
      i += 1;
      continue;
    }

    if (token === '--password') {
      request.password = String(tokens[i + 1] || '');
      i += 1;
      continue;
    }

    if (token === '--regex') {
      request.regex = true;
      continue;
    }

    if (token === '--wholeword' || token === '--whole-word') {
      request.wholeWord = true;
      continue;
    }

    if (token === '--case' || token === '--match-case') {
      request.matchCase = true;
      continue;
    }

    if (token.startsWith('--')) {
      errors.push(`未知参数: ${token}`);
      continue;
    }

    queryParts.push(token);
  }

  request.query = queryParts.join(' ').trim();
  if (request.query) {
    const textWithoutMarker = originalText.replace(/\/everything|\/es/ig, ' ').trim();
    const naturalFromQuery = extractQueryFromNaturalText(request.query);
    const naturalFromWhole = extractQueryFromNaturalText(textWithoutMarker);

    const normalizedQuery = cleanupAutoQuery(request.query);
    const looksLikeNaturalSentence = /(?:找|查|搜索|检索|定位|在哪|帮我|请|文件|目录|路径|find|search|locate)/i.test(normalizedQuery)
      && /\s|，|。|？|\?|！|!/.test(normalizedQuery);

    const preferred = naturalFromQuery || naturalFromWhole;
    if (preferred && (looksLikeNaturalSentence || preferred.length < normalizedQuery.length)) {
      request.query = preferred;
    } else {
      request.query = normalizedQuery;
    }
  }

  if (!request.query) {
    const textWithoutMarker = originalText.replace(/\/everything|\/es/ig, ' ').trim();
    const fallbackQuery = extractQueryFromNaturalText(textWithoutMarker) || extractQueryFromNaturalText(originalText);
    if (fallbackQuery) {
      request.query = fallbackQuery;
    } else {
      errors.push('缺少查询词');
    }
  }

  return {
    invoked: true,
    command,
    mode: 'command',
    request,
    followUp: followUp || originalText,
    errors,
  };
}

function hasEverythingAutoOptOut(text) {
  const normalized = String(text || '').toLowerCase();
  const hints = ['不要搜索', '别搜索', '不用搜索', '不需要搜索', '不要检索', '不用检索', '不需要检索', '不要调用everything'];
  return hints.some((hint) => normalized.includes(hint));
}

function inferEverythingScopeFromText(text, fallback = 'file') {
  const source = String(text || '').toLowerCase();
  if (
    source.includes('目录')
    || source.includes('路径')
    || source.includes('folder')
    || source.includes('path')
    || source.includes('在哪个文件夹')
    || /[a-z]:\\/i.test(source)
    || source.includes('\\')
    || source.includes('/')
  ) {
    return 'path';
  }
  return fallback;
}

function cleanupAutoQuery(text) {
  let query = String(text || '').trim();
  query = query.replace(/^["'“”‘’`]+|["'“”‘’`]+$/g, '').trim();
  query = query.replace(/[。！？!?,，；;]+$/g, '').trim();
  query = query.replace(/^(请|麻烦|帮我|帮忙|可以|能不能|是否)\s*/g, '').trim();
  if (query.length > 120) {
    query = query.slice(0, 120).trim();
  }
  return query;
}

function extractQueryFromNaturalText(text) {
  const raw = String(text || '').trim();
  if (!raw) {
    return '';
  }

  const quoted = Array.from(raw.matchAll(/["“”'‘’]([^"“”'‘’]{1,120})["“”'‘’]/g)).map((m) => m[1].trim()).filter(Boolean);
  if (quoted.length > 0) {
    return cleanupAutoQuery(quoted[0]);
  }

  const fileLike = raw.match(/([A-Za-z0-9_\-\.]+\.[A-Za-z0-9]{1,12})/);
  if (fileLike?.[1]) {
    return cleanupAutoQuery(fileLike[1]);
  }

  const pathLike = raw.match(/([A-Za-z]:\\[^\s]+|(?:[A-Za-z0-9_.-]+\\)+[A-Za-z0-9_.-]+|(?:[A-Za-z0-9_.-]+\/)+[A-Za-z0-9_.-]+)/);
  if (pathLike?.[1]) {
    return cleanupAutoQuery(pathLike[1]);
  }

  const afterAction = raw.match(/(?:找|查|搜索|检索|定位|find|search|locate)\s*(?:一下|下)?\s*(?:文件|目录|路径|file|folder|path)?\s*[:：]?\s*(.+)$/i);
  if (afterAction?.[1]) {
    return cleanupAutoQuery(afterAction[1]);
  }

  const afterQuestion = raw.match(/(?:在哪|在哪里|在哪个目录|在哪个文件夹)\s*[:：]?\s*(.+)$/i);
  if (afterQuestion?.[1]) {
    return cleanupAutoQuery(afterQuestion[1]);
  }

  return '';
}

function parseEverythingSkillAuto(userText) {
  if (!EVERYTHING_AUTO_TRIGGER_ENABLED) {
    return null;
  }

  const raw = String(userText || '').trim();
  if (!raw || hasEverythingAutoOptOut(raw)) {
    return null;
  }

  const lowered = raw.toLowerCase();
  const hasActionIntent = /找|查|搜索|检索|定位|在哪|where|find|search|locate/.test(lowered);
  const hasTargetHint = /文件|文件名|目录|路径|folder|file|path|\.[a-z0-9]{1,10}\b|[a-z]:\\|\\|\//i.test(raw);

  if (!hasActionIntent || !hasTargetHint) {
    return null;
  }

  const query = extractQueryFromNaturalText(raw);
  if (!query || query.length < 2) {
    return null;
  }

  return {
    invoked: true,
    command: 'auto',
    mode: 'auto',
    request: {
      query,
      scope: inferEverythingScopeFromText(raw, 'file'),
      count: 30,
      offset: 0,
      regex: false,
      wholeWord: false,
      matchCase: false,
    },
    followUp: raw,
    errors: [],
  };
}

function buildEverythingSkillErrorResponse(request, message) {
  return {
    ok: false,
    query: String(request?.query || ''),
    scope: String(request?.scope || 'file'),
    endpoint: request?.host ? `http://${request.host}:${request?.port || 80}/` : 'http://127.0.0.1:80/',
    returned: 0,
    total: null,
    results: [],
    text: String(message || 'Everything skill 调用失败'),
  };
}

async function resolveEverythingSkillContext(userText) {
  const parsed = parseEverythingSkillCommand(userText) || parseEverythingSkillAuto(userText);
  if (!parsed) {
    return { invoked: false };
  }

  if (parsed.errors.length > 0) {
    return {
      invoked: true,
      request: parsed.request,
      mode: parsed.mode,
      followUp: parsed.followUp,
      response: buildEverythingSkillErrorResponse(
        parsed.request,
        `Everything skill 参数错误: ${parsed.errors.join('；')}。示例: /everything "CCConnect" --scope file --count 20 --ask 帮我找最相关文件`,
      ),
    };
  }

  try {
    const response = await runRustEverythingSearch(parsed.request);
    if (!response || typeof response !== 'object') {
      return {
        invoked: true,
        request: parsed.request,
        mode: parsed.mode,
        followUp: parsed.followUp,
        response: buildEverythingSkillErrorResponse(parsed.request, 'Everything skill 返回格式无效。'),
      };
    }
    return {
      invoked: true,
      request: parsed.request,
      mode: parsed.mode,
      followUp: parsed.followUp,
      response,
    };
  } catch (error) {
    return {
      invoked: true,
      request: parsed.request,
      mode: parsed.mode,
      followUp: parsed.followUp,
      response: buildEverythingSkillErrorResponse(
        parsed.request,
        `Everything skill 调用失败: ${error?.message || String(error)}`,
      ),
    };
  }
}

function buildEverythingSkillPromptLines(skillContext) {
  if (!skillContext?.invoked) {
    return [];
  }

  const req = skillContext.request || {};
  const resp = skillContext.response || {};
  const mode = String(skillContext.mode || 'command');
  const lines = [
    '',
    '以下是系统刚执行的一次 Claude Skill 调用结果，请优先依据它回答。',
    'skill: everything.search',
    `skill trigger: ${mode}`,
    `skill request: query="${String(req.query || '')}" scope=${String(req.scope || 'file')} count=${Number(req.count || 0)} offset=${Number(req.offset || 0)}`,
  ];

  if (resp.ok) {
    lines.push(`skill status: ok (returned=${Number(resp.returned || 0)}, total=${resp.total ?? 'unknown'})`);
    lines.push(`skill endpoint: ${String(resp.endpoint || '')}`);
    lines.push('skill top results:');

    const results = Array.isArray(resp.results) ? resp.results.slice(0, EVERYTHING_SKILL_MAX_RESULTS_IN_PROMPT) : [];
    if (results.length === 0) {
      lines.push('- (无匹配结果)');
    } else {
      results.forEach((item, index) => {
        const fullPath = String(item?.fullPath || '').trim() || String(item?.path || '').trim();
        const name = String(item?.name || '').trim();
        const line = fullPath ? `${index + 1}. ${fullPath}` : `${index + 1}. ${name || '(unnamed)'}`;
        lines.push(line);
      });
    }
  } else {
    lines.push(`skill status: failed`);
    lines.push(`skill error: ${String(resp.text || '未知错误')}`);
  }

  return lines;
}

function extractSortCommandSegment(text) {
  const raw = String(text || '');
  if (!raw.trim()) {
    return '';
  }

  const markerMatch = /(\/sort)\b/i.exec(raw);
  if (!markerMatch) {
    return '';
  }

  const commandText = markerMatch[1] || '';
  const markerIndex = Number(markerMatch.index) || 0;
  let rest = raw.slice(markerIndex + commandText.length);
  if (rest && !/^\s/.test(rest)) {
    rest = ` ${rest}`;
  }

  return `${commandText}${rest}`.trim();
}

function normalizeSortStrategy(value) {
  const lowered = String(value || '').trim().toLowerCase();
  if (!lowered) {
    return 'byAi';
  }

  if (['ai', 'byai', 'llm', 'smart', '智能'].includes(lowered)) {
    return 'byAi';
  }
  if (['project', 'projects', 'byproject', '按项目'].includes(lowered)) {
    return 'byProject';
  }
  if (['date', 'bydate', '按日期', '时间'].includes(lowered)) {
    return 'byDate';
  }
  return 'byType';
}

function inferSortBaseDirFromText(text) {
  const source = String(text || '').toLowerCase();
  if (!source) {
    return '';
  }
  if (source.includes('桌面') || source.includes('desktop')) {
    return 'desktop';
  }
  if (source.includes('下载') || source.includes('download')) {
    return 'downloads';
  }
  if (source.includes('文档') || source.includes('documents') || source.includes('docs')) {
    return 'documents';
  }
  if (source.includes('当前目录') || source.includes('workspace') || source.includes('current')) {
    return 'current';
  }
  return '';
}

function resolveSortRequestedDir(baseDirToken, settings) {
  const token = String(baseDirToken || '').trim();
  const configuredDir = String(settings?.workingDirectory || '').trim();

  if (!token) {
    return configuredDir;
  }

  const lowered = token.toLowerCase();
  if (lowered === 'desktop' || lowered === '桌面') {
    return configuredDir || token;
  }

  if (lowered === 'current' || lowered === 'workspace' || lowered === '当前目录') {
    return configuredDir || token;
  }

  return token;
}

const SORT_CATEGORY_ALIASES = Object.freeze({
  projects: 'Projects',
  project: 'Projects',
  '项目': 'Projects',
  documents: 'Documents',
  document: 'Documents',
  docs: 'Documents',
  '文档': 'Documents',
  '资料': 'Documents',
  images: 'Images',
  image: 'Images',
  '图片': 'Images',
  videos: 'Videos',
  video: 'Videos',
  '视频': 'Videos',
  music: 'Music',
  audio: 'Music',
  audios: 'Music',
  '音频': 'Music',
  '音乐': 'Music',
  archives: 'Archives',
  archive: 'Archives',
  '压缩包': 'Archives',
  '归档': 'Archives',
  code: 'Code',
  '代码': 'Code',
  others: 'Others',
  other: 'Others',
  '其他': 'Others',
});

function normalizeSortFocus(value) {
  const raw = String(value || '').trim();
  if (!raw) {
    return '';
  }

  const lowered = raw.toLowerCase();
  if (['all', 'any', '全部', '所有'].includes(lowered)) {
    return 'ALL';
  }

  return SORT_CATEGORY_ALIASES[lowered] || SORT_CATEGORY_ALIASES[raw] || '';
}

function inferSortFocusFromObjective(text) {
  const source = String(text || '').trim();
  if (!source) {
    return '';
  }

  if (/压缩包|压缩文件|归档|zip|rar|7z|tar|gz|tgz/i.test(source)) {
    return 'Archives';
  }
  if (/图片|照片|截图|壁纸|image|photo|png|jpg|jpeg|webp|gif/i.test(source)) {
    return 'Images';
  }
  if (/视频|video|mp4|mkv|mov|avi|wmv/i.test(source)) {
    return 'Videos';
  }
  if (/音频|音乐|audio|music|mp3|wav|flac|m4a|aac/i.test(source)) {
    return 'Music';
  }
  if (/文档|资料|document|docs|pdf|docx?|xlsx?|pptx?|txt|md/i.test(source)) {
    return 'Documents';
  }
  if (/代码|源码|code|program|script|rs|py|js|ts|java|c\+\+|cpp/i.test(source)) {
    return 'Code';
  }
  if (/项目|工程|project/i.test(source)) {
    return 'Projects';
  }

  return '';
}

function inferSortExtensionsFromObjective(text) {
  const source = String(text || '').trim().toLowerCase();
  if (!source) {
    return [];
  }

  const result = new Set();
  if (/\bpptx?\b|powerpoint|幻灯片|演示文稿|投影片/.test(source)) {
    result.add('ppt');
    result.add('pptx');
  }

  const explicitExts = source.match(/\.[a-z0-9]{1,10}\b/g) || [];
  for (const extToken of explicitExts) {
    const normalized = String(extToken || '').replace('.', '').trim();
    if (normalized) {
      result.add(normalized);
    }
  }

  return Array.from(result);
}

function filterSortEntriesByExtensions(entries, extensions) {
  const normalizedEntries = (Array.isArray(entries) ? entries : [])
    .map(normalizeSortEntryPayload)
    .filter(Boolean);

  const normalizedExts = Array.from(new Set((Array.isArray(extensions) ? extensions : [])
    .map((v) => String(v || '').trim().toLowerCase())
    .filter(Boolean)));

  if (normalizedExts.length === 0) {
    return {
      entries: normalizedEntries,
      extensions: [],
      removed: 0,
    };
  }

  const extSet = new Set(normalizedExts);
  const scoped = normalizedEntries.filter((entry) => extSet.has(String(entry.extension || '').toLowerCase()));
  return {
    entries: scoped,
    extensions: normalizedExts,
    removed: Math.max(0, normalizedEntries.length - scoped.length),
  };
}

function inferSortSingleFolderName(objective, extensions) {
  const source = String(objective || '').toLowerCase();
  const exts = Array.isArray(extensions) ? extensions : [];

  if (exts.includes('ppt') || exts.includes('pptx') || /ppt|powerpoint|幻灯片|演示文稿/.test(source)) {
    return 'PPT';
  }

  if (exts.length === 1) {
    return String(exts[0]).toUpperCase();
  }

  return '分类结果';
}

function shouldUseSingleFolderFallback(objective, extensions) {
  const source = String(objective || '').toLowerCase();
  if ((Array.isArray(extensions) ? extensions.length : 0) > 0) {
    return true;
  }

  return /一个文件夹|同一个文件夹|放到一个文件夹|放在一个文件夹|归到一个文件夹|单独文件夹/.test(source);
}

function getSortFocusDisplayName(focus) {
  switch (focus) {
    case 'Projects': return '项目文件';
    case 'Documents': return '文档资料';
    case 'Images': return '图片素材';
    case 'Videos': return '视频文件';
    case 'Music': return '音频文件';
    case 'Archives': return '压缩包';
    case 'Code': return '代码文件';
    case 'Others': return '其他文件';
    default: return '全量文件';
  }
}

function buildSortPreviewFromPlan(plan, limit = 50) {
  const sourcePlan = plan && typeof plan === 'object' ? plan : {};
  const operations = Array.isArray(sourcePlan.operations) ? sourcePlan.operations : [];
  const rows = [];

  for (const op of operations) {
    const item = op && typeof op === 'object' ? op : {};
    if (item.type === 'createDir') {
      rows.push(`mkdir ${String(item.path || '')}`);
      continue;
    }
    if (item.type === 'moveFile') {
      rows.push(`move ${String(item.from || '')} -> ${String(item.to || '')}`);
      continue;
    }
    if (item.type === 'skip') {
      rows.push(`skip ${String(item.path || '')} (${String(item.reason || 'skip')})`);
    }
  }

  return rows.slice(0, Math.max(1, Number(limit) || 50));
}

function filterSortPlanByFocus(plan, focusCategory) {
  const focus = normalizeSortFocus(focusCategory);
  const srcPlan = plan && typeof plan === 'object' ? plan : {};
  const operations = Array.isArray(srcPlan.operations) ? srcPlan.operations : [];

  if (!focus || focus === 'ALL') {
    const originalMoves = operations.filter((op) => String(op?.type || '') === 'moveFile').length;
    return { plan: srcPlan, keptMoves: originalMoves, removedMoves: 0, focus: '' };
  }

  const normalizePathText = (text) => String(text || '')
    .replace(/\//g, '\\')
    .replace(/\\+/g, '\\')
    .toLowerCase();
  const inFocus = (pathText) => normalizePathText(pathText).includes(`\\${String(focus).toLowerCase()}\\`);

  const keptMoveOps = operations.filter((op) => String(op?.type || '') === 'moveFile' && inFocus(op?.to));
  const neededDirs = new Set(
    keptMoveOps
      .map((op) => path.dirname(String(op?.to || '')))
      .filter((v) => String(v || '').trim())
      .map((v) => normalizePathText(v)),
  );

  const nextOps = [];
  for (const op of operations) {
    const type = String(op?.type || '');
    if (type === 'moveFile') {
      if (inFocus(op?.to)) {
        nextOps.push(op);
      }
      continue;
    }

    if (type === 'createDir') {
      const dirText = normalizePathText(op?.path);
      const required = Array.from(neededDirs).some((targetDir) => targetDir === dirText || targetDir.startsWith(`${dirText}\\`));
      if (required) {
        nextOps.push(op);
      }
      continue;
    }
  }

  const originalMoves = operations.filter((op) => String(op?.type || '') === 'moveFile').length;
  const removedMoves = Math.max(0, originalMoves - keptMoveOps.length);
  const nextSummary = srcPlan.summary && typeof srcPlan.summary === 'object'
    ? {
      ...srcPlan.summary,
      plannedMoves: keptMoveOps.length,
      skipped: Number(srcPlan.summary.skipped || 0) + removedMoves,
    }
    : srcPlan.summary;

  return {
    plan: {
      ...srcPlan,
      operations: nextOps,
      summary: nextSummary,
    },
    keptMoves: keptMoveOps.length,
    removedMoves,
    focus,
  };
}

function sanitizeFolderName(name) {
  const raw = String(name || '').trim();
  if (!raw) {
    return '';
  }
  const cleaned = raw
    .replace(/[<>:"/\\|?*\x00-\x1F]/g, '_')
    .replace(/[.\s]+$/g, '')
    .trim();
  return cleaned;
}

function buildSortRenameMap(rawRenames) {
  const input = rawRenames && typeof rawRenames === 'object' ? rawRenames : {};
  const map = {};

  for (const [rawKey, rawValue] of Object.entries(input)) {
    const key = String(rawKey || '').trim();
    if (!key) {
      continue;
    }

    const lowerKey = key.toLowerCase();
    const canonical = SORT_CATEGORY_ALIASES[lowerKey]
      || SORT_CATEGORY_ALIASES[key]
      || (['Projects', 'Documents', 'Images', 'Videos', 'Music', 'Archives', 'Code', 'Others'].includes(key)
        ? key
        : '');
    if (!canonical) {
      continue;
    }

    const folderName = sanitizeFolderName(rawValue);
    if (!folderName) {
      continue;
    }

    map[canonical] = folderName;
  }

  return map;
}

function applySortFolderRenamesToPath(pathText, baseDir, renameMap) {
  const source = String(pathText || '').trim();
  const base = String(baseDir || '').trim();
  if (!source || !base || !renameMap || Object.keys(renameMap).length === 0) {
    return source;
  }

  let relative = '';
  try {
    relative = path.relative(base, source);
  } catch {
    return source;
  }

  if (!relative || relative.startsWith('..') || path.isAbsolute(relative)) {
    return source;
  }

  const parts = relative.split(/[\\/]+/).filter(Boolean);
  if (parts.length === 0) {
    return source;
  }

  const first = parts[0];
  const mapped = renameMap[first];
  if (!mapped) {
    return source;
  }

  parts[0] = mapped;
  return path.join(base, ...parts);
}

function applySortFolderRenamesToPlan(plan, baseDir, renameMap) {
  const srcPlan = plan && typeof plan === 'object' ? plan : {};
  const operations = Array.isArray(srcPlan.operations) ? srcPlan.operations : [];

  const nextOps = operations.map((op) => {
    const item = op && typeof op === 'object' ? { ...op } : {};
    if (item.type === 'createDir' && item.path) {
      item.path = applySortFolderRenamesToPath(item.path, baseDir, renameMap);
    }
    if (item.type === 'moveFile' && item.to) {
      item.to = applySortFolderRenamesToPath(item.to, baseDir, renameMap);
    }
    return item;
  });

  return {
    ...srcPlan,
    operations: nextOps,
  };
}

function normalizeSortRelativePath(input) {
  const raw = String(input || '').trim();
  if (!raw) {
    return '';
  }

  const cleaned = raw
    .replace(/^\.[\\/]/, '')
    .replace(/^[/\\]+/, '')
    .replace(/[\\/]+/g, '\\')
    .trim();

  if (!cleaned) {
    return '';
  }

  const parts = cleaned.split('\\').filter(Boolean);
  if (parts.length === 0 || parts.some((part) => part === '..')) {
    return '';
  }

  return parts.join('\\');
}

function normalizeSortFsPath(input) {
  return String(input || '')
    .replace(/[\\/]+/g, '\\')
    .replace(/\\+$/g, '')
    .toLowerCase();
}

function normalizeSortEntryPayload(input) {
  const item = input && typeof input === 'object' ? input : null;
  if (!item) {
    return null;
  }

  const relativePath = normalizeSortRelativePath(item.relativePath || item.path);
  if (!relativePath) {
    return null;
  }

  const name = String(item.name || path.basename(relativePath)).trim();
  if (!name) {
    return null;
  }

  return {
    relativePath,
    name,
    extension: String(item.extension || '').trim().toLowerCase(),
    category: String(item.category || 'Other').trim(),
    sizeBytes: Math.max(0, Number(item.sizeBytes || 0) || 0),
    modified: String(item.modified || '').trim(),
  };
}

function createEmptySortSummary() {
  return {
    totalFiles: 0,
    totalSizeBytes: 0,
    projects: 0,
    documents: 0,
    images: 0,
    videos: 0,
    audios: 0,
    archives: 0,
    code: 0,
    others: 0,
    plannedMoves: 0,
    skipped: 0,
  };
}

function applySortCategoryCounter(summary, category) {
  const normalized = String(category || '').trim().toLowerCase();
  if (normalized === 'project') {
    summary.projects += 1;
    return;
  }
  if (normalized === 'document') {
    summary.documents += 1;
    return;
  }
  if (normalized === 'image') {
    summary.images += 1;
    return;
  }
  if (normalized === 'video') {
    summary.videos += 1;
    return;
  }
  if (normalized === 'audio') {
    summary.audios += 1;
    return;
  }
  if (normalized === 'archive') {
    summary.archives += 1;
    return;
  }
  if (normalized === 'code') {
    summary.code += 1;
    return;
  }
  summary.others += 1;
}

function summarizeSortEntries(entries, plannedMoves, skipped) {
  const summary = createEmptySortSummary();
  const list = Array.isArray(entries) ? entries : [];

  for (const entry of list) {
    summary.totalFiles += 1;
    summary.totalSizeBytes += Math.max(0, Number(entry?.sizeBytes || 0) || 0);
    applySortCategoryCounter(summary, entry?.category);
  }

  summary.plannedMoves = Math.max(0, Number(plannedMoves || 0) || 0);
  summary.skipped = Math.max(0, Number(skipped || 0) || 0);
  return summary;
}

function sortEntryMatchesFocus(entry, focus) {
  if (!focus || focus === 'ALL') {
    return true;
  }

  const category = String(entry?.category || '').trim().toLowerCase();
  switch (focus) {
    case 'Projects': return category === 'project';
    case 'Documents': return category === 'document';
    case 'Images': return category === 'image';
    case 'Videos': return category === 'video';
    case 'Music': return category === 'audio';
    case 'Archives': return category === 'archive';
    case 'Code': return category === 'code';
    case 'Others': return category === 'other';
    default: return true;
  }
}

function filterSortEntriesByFocus(entries, focusCategory) {
  const normalizedFocus = normalizeSortFocus(focusCategory);
  const normalizedEntries = (Array.isArray(entries) ? entries : [])
    .map(normalizeSortEntryPayload)
    .filter(Boolean);

  if (!normalizedFocus || normalizedFocus === 'ALL') {
    return {
      entries: normalizedEntries,
      focus: '',
      removed: 0,
    };
  }

  const focusedEntries = normalizedEntries.filter((entry) => sortEntryMatchesFocus(entry, normalizedFocus));
  return {
    entries: focusedEntries,
    focus: normalizedFocus,
    removed: Math.max(0, normalizedEntries.length - focusedEntries.length),
  };
}

function buildSortAiGroupingPrompt({ userText, objective, requestedDir, focus, includeShortcuts, entries }) {
  const candidateEntries = (Array.isArray(entries) ? entries : []).slice(0, SORT_AI_MAX_PROMPT_ENTRIES);
  const entryPayload = candidateEntries.map((item, index) => ({
    id: index + 1,
    relativePath: item.relativePath,
    fileName: item.name,
    extension: item.extension,
    category: item.category,
    sizeBytes: item.sizeBytes,
  }));

  const lines = [
    '你是文件整理规划助手。请基于给定文件清单，输出可执行的分类目录与文件归位结果。',
    `只输出 JSON，并包裹在 <${SORT_AI_PLAN_MARKER}>...</${SORT_AI_PLAN_MARKER}>。`,
    'JSON 结构:',
    '{"outline":["步骤1"],"groups":[{"name":"学习资料","files":["sub\\\\a.pdf"]}],"notes":["..."]}',
    '约束:',
    '1) files 里只能引用给定 relativePath；不要凭空编造。',
    '2) 每个文件最多出现一次。',
    '3) 目录名必须是短名称，不要包含路径分隔符。',
    '4) 若用户明确指定了文件类型/范围（如 PPT、图片、压缩包），只处理该范围文件；未命中的文件不要强行分配。',
    '5) 若用户未限定范围，再做全量分类，分类需清晰、可维护。',
    '',
    `用户原始消息: ${String(userText || '').trim()}`,
    `目标描述: ${String(objective || '').trim()}`,
    `目标目录: ${String(requestedDir || '').trim()}`,
    `聚焦类别: ${focus ? String(focus) : 'ALL'}`,
    `是否包含快捷方式: ${includeShortcuts ? 'yes' : 'no'}`,
    `候选文件数量: ${candidateEntries.length}`,
    '',
    '候选文件(JSON):',
    JSON.stringify(entryPayload, null, 2),
  ];

  return lines.join('\n');
}

function stripCodeFence(text) {
  const source = String(text || '').trim();
  return source
    .replace(/^```(?:json)?\s*/i, '')
    .replace(/\s*```$/i, '')
    .trim();
}

function extractFirstJsonObject(text) {
  const source = String(text || '');
  const start = source.indexOf('{');
  if (start < 0) {
    return '';
  }

  let depth = 0;
  let inString = false;
  let quote = '';
  let escaped = false;

  for (let i = start; i < source.length; i += 1) {
    const ch = source[i];
    if (inString) {
      if (escaped) {
        escaped = false;
        continue;
      }
      if (ch === '\\') {
        escaped = true;
        continue;
      }
      if (ch === quote) {
        inString = false;
        quote = '';
      }
      continue;
    }

    if (ch === '"' || ch === "'") {
      inString = true;
      quote = ch;
      continue;
    }

    if (ch === '{') {
      depth += 1;
      continue;
    }

    if (ch === '}') {
      depth -= 1;
      if (depth === 0) {
        return source.slice(start, i + 1).trim();
      }
    }
  }

  return '';
}

function sanitizeJsonCandidate(text) {
  return stripCodeFence(text)
    .replace(/[\u200B-\u200D\uFEFF]/g, '')
    .replace(/[\u201C\u201D]/g, '"')
    .replace(/[\u2018\u2019]/g, "'")
    .replace(/,\s*([}\]])/g, '$1')
    .trim();
}

function tryParseJsonWithFallback(candidates) {
  const queue = Array.isArray(candidates) ? candidates : [];
  for (const candidate of queue) {
    const source = String(candidate || '').trim();
    if (!source) {
      continue;
    }

    const attempts = [
      source,
      stripCodeFence(source),
      extractFirstJsonObject(source),
      sanitizeJsonCandidate(source),
      sanitizeJsonCandidate(extractFirstJsonObject(source)),
    ].filter(Boolean);

    for (const attempt of attempts) {
      try {
        return JSON.parse(attempt);
      } catch {
        // Keep trying additional candidates.
      }
    }
  }

  return null;
}

function parseSortGroupingFromAiText(text) {
  const source = String(text || '').replace(/[\u200B-\u200D\uFEFF]/g, '');
  const markerRegex = new RegExp(`<${SORT_AI_PLAN_MARKER}>\\s*([\\s\\S]*?)\\s*</${SORT_AI_PLAN_MARKER}>`, 'i');
  const marker = source.match(markerRegex);
  const fenced = source.match(/```(?:json)?\s*([\s\S]*?)\s*```/i);
  const parsed = tryParseJsonWithFallback([
    marker?.[1] ? String(marker[1]).trim() : '',
    fenced?.[1] ? String(fenced[1]).trim() : '',
    source,
  ]);

  if (!parsed || typeof parsed !== 'object') {
    return { ok: false };
  }

  try {
    const outline = Array.isArray(parsed?.outline)
      ? parsed.outline.map((v) => String(v || '').trim()).filter(Boolean).slice(0, 10)
      : [];
    const notes = Array.isArray(parsed?.notes)
      ? parsed.notes.map((v) => String(v || '').trim()).filter(Boolean).slice(0, 10)
      : [];

    const groups = [];
    if (Array.isArray(parsed?.groups)) {
      for (const rawGroup of parsed.groups) {
        const name = sanitizeFolderName(rawGroup?.name);
        if (!name) {
          continue;
        }

        const files = Array.isArray(rawGroup?.files)
          ? rawGroup.files.map((item) => normalizeSortRelativePath(item)).filter(Boolean)
          : [];
        if (files.length === 0) {
          continue;
        }

        groups.push({
          name,
          files: Array.from(new Set(files)),
        });
      }
    }

    if (groups.length === 0 && parsed?.assignments && typeof parsed.assignments === 'object') {
      const bucket = new Map();
      for (const [rawPath, rawFolder] of Object.entries(parsed.assignments)) {
        const rel = normalizeSortRelativePath(rawPath);
        const folder = sanitizeFolderName(rawFolder);
        if (!rel || !folder) {
          continue;
        }
        if (!bucket.has(folder)) {
          bucket.set(folder, []);
        }
        bucket.get(folder).push(rel);
      }

      for (const [name, relPaths] of bucket.entries()) {
        groups.push({
          name,
          files: Array.from(new Set(relPaths)),
        });
      }
    }

    if (groups.length === 0 && parsed?.filesByFolder && typeof parsed.filesByFolder === 'object') {
      for (const [rawFolder, rawFiles] of Object.entries(parsed.filesByFolder)) {
        const name = sanitizeFolderName(rawFolder);
        if (!name) {
          continue;
        }

        const files = Array.isArray(rawFiles)
          ? rawFiles.map((item) => normalizeSortRelativePath(item)).filter(Boolean)
          : [];
        if (files.length === 0) {
          continue;
        }

        groups.push({
          name,
          files: Array.from(new Set(files)),
        });
      }
    }

    if (groups.length === 0 && Array.isArray(parsed?.fileGroups)) {
      for (const item of parsed.fileGroups) {
        const name = sanitizeFolderName(item?.name || item?.folder || item?.group);
        if (!name) {
          continue;
        }

        const files = Array.isArray(item?.files)
          ? item.files.map((v) => normalizeSortRelativePath(v)).filter(Boolean)
          : [];
        if (files.length === 0) {
          continue;
        }

        groups.push({
          name,
          files: Array.from(new Set(files)),
        });
      }
    }

    if (groups.length === 0) {
      return { ok: false };
    }

    return {
      ok: true,
      outline,
      notes,
      groups,
    };
  } catch {
    return { ok: false };
  }
}

function ensureUniqueSortTargetPath(desiredPath, reservedPaths) {
  const normalizedDesired = normalizeSortFsPath(desiredPath);
  if (!reservedPaths.has(normalizedDesired) && !fs.existsSync(desiredPath)) {
    reservedPaths.add(normalizedDesired);
    return desiredPath;
  }

  const parsed = path.parse(desiredPath);
  for (let idx = 1; idx <= 9999; idx += 1) {
    const candidate = path.join(parsed.dir, `${parsed.name} (${idx})${parsed.ext}`);
    const normalizedCandidate = normalizeSortFsPath(candidate);
    if (reservedPaths.has(normalizedCandidate) || fs.existsSync(candidate)) {
      continue;
    }
    reservedPaths.add(normalizedCandidate);
    return candidate;
  }

  reservedPaths.add(normalizedDesired);
  return desiredPath;
}

function buildSortPlanFromAiGroups({ baseDir, entries, groups }) {
  const normalizedBaseDir = String(baseDir || '').trim();
  const normalizedEntries = (Array.isArray(entries) ? entries : [])
    .map(normalizeSortEntryPayload)
    .filter(Boolean);
  const normalizedGroups = Array.isArray(groups) ? groups : [];

  const entryByRelativePath = new Map();
  for (const entry of normalizedEntries) {
    entryByRelativePath.set(entry.relativePath.toLowerCase(), entry);
  }

  const usedEntries = new Set();
  const createdDirs = new Set();
  const reservedTargets = new Set();
  const operations = [];
  const folderUsage = new Map();

  for (const group of normalizedGroups) {
    const folderName = sanitizeFolderName(group?.name);
    if (!folderName) {
      continue;
    }

    const targetDir = path.join(normalizedBaseDir, folderName);
    const targetDirKey = normalizeSortFsPath(targetDir);
    if (!createdDirs.has(targetDirKey)) {
      operations.push({ type: 'createDir', path: targetDir });
      createdDirs.add(targetDirKey);
    }

    const files = Array.isArray(group?.files) ? group.files : [];
    for (const rawRelPath of files) {
      const relPath = normalizeSortRelativePath(rawRelPath);
      if (!relPath) {
        continue;
      }

      const key = relPath.toLowerCase();
      if (usedEntries.has(key)) {
        continue;
      }

      const entry = entryByRelativePath.get(key);
      if (!entry) {
        continue;
      }

      const fromPath = path.join(normalizedBaseDir, ...entry.relativePath.split('\\'));
      const desiredPath = path.join(targetDir, entry.name);
      const targetPath = ensureUniqueSortTargetPath(desiredPath, reservedTargets);

      if (normalizeSortFsPath(fromPath) === normalizeSortFsPath(targetPath)) {
        operations.push({ type: 'skip', path: fromPath, reason: 'already in target' });
        usedEntries.add(key);
        continue;
      }

      operations.push({
        type: 'moveFile',
        from: fromPath,
        to: targetPath,
      });

      usedEntries.add(key);
      folderUsage.set(folderName, (folderUsage.get(folderName) || 0) + 1);
    }
  }

  for (const entry of normalizedEntries) {
    const key = entry.relativePath.toLowerCase();
    if (usedEntries.has(key)) {
      continue;
    }
    operations.push({
      type: 'skip',
      path: path.join(normalizedBaseDir, ...entry.relativePath.split('\\')),
      reason: 'ai-unassigned',
    });
  }

  const moveCount = operations.filter((op) => String(op?.type || '') === 'moveFile').length;
  const skipCount = operations.filter((op) => String(op?.type || '') === 'skip').length;
  const summary = summarizeSortEntries(normalizedEntries, moveCount, skipCount);
  const folders = Array.from(folderUsage.entries())
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
    .map(([name, count]) => ({ name, count }));

  return {
    plan: {
      operations,
      summary,
    },
    assigned: moveCount,
    unassigned: skipCount,
    folders,
  };
}

function buildSortAiPrompt({ userText, objective, requestedDir, strategy, summary, preview, focus, includeShortcuts }) {
  const lines = [
    '你是文件整理规划助手。请根据用户意图设计整理大纲和分类目录命名。',
    '只输出 JSON，并包裹在 <SORT_DESIGN_JSON>...</SORT_DESIGN_JSON>。',
    'JSON结构:',
    '{"outline":["步骤1", "步骤2"], "renames":{"Documents":"学习资料","Images":"图片素材"}, "notes":["..."]}',
    '约束:',
    '1) renames 只允许以下键: Projects, Documents, Images, Videos, Music, Archives, Code, Others',
    '2) 值必须是目录名，不含路径分隔符。',
    '3) 不要输出多余解释。',
    '',
    `用户原始消息: ${String(userText || '').trim()}`,
    `目标描述: ${String(objective || '').trim()}`,
    `目标目录: ${String(requestedDir || '').trim()}`,
    `默认策略: ${String(strategy || 'byType')}`,
    `聚焦类别: ${focus ? String(focus) : 'ALL'}`,
    `是否包含快捷方式: ${includeShortcuts ? 'yes' : 'no'}`,
  ];

  if (summary && typeof summary === 'object') {
    lines.push(
      `扫描摘要: files=${Number(summary.totalFiles || 0)}, moves=${Number(summary.plannedMoves || 0)}, skipped=${Number(summary.skipped || 0)}`,
    );
  }

  const previewLines = Array.isArray(preview) ? preview.slice(0, 25) : [];
  if (previewLines.length > 0) {
    lines.push('', '计划预览(裁剪):');
    previewLines.forEach((line) => lines.push(`- ${String(line || '').slice(0, 220)}`));
  }

  return lines.join('\n');
}

function parseSortDesignFromAiText(text) {
  const source = String(text || '').replace(/[\u200B-\u200D\uFEFF]/g, '');
  const marker = source.match(/<SORT_DESIGN_JSON>\s*([\s\S]*?)\s*<\/SORT_DESIGN_JSON>/i);
  const jsonText = marker?.[1] ? String(marker[1]).trim() : '';
  if (!jsonText) {
    return { ok: false };
  }

  try {
    const parsed = JSON.parse(jsonText);
    const outline = Array.isArray(parsed?.outline)
      ? parsed.outline.map((v) => String(v || '').trim()).filter(Boolean).slice(0, 8)
      : [];
    const notes = Array.isArray(parsed?.notes)
      ? parsed.notes.map((v) => String(v || '').trim()).filter(Boolean).slice(0, 8)
      : [];
    const renameMap = buildSortRenameMap(parsed?.renames);

    return {
      ok: true,
      outline,
      notes,
      renameMap,
    };
  } catch {
    return { ok: false };
  }
}

function normalizeSortOperationForLog(op) {
  const item = op && typeof op === 'object' ? op : {};
  const type = String(item.type || '').trim();
  if (!type) {
    return null;
  }

  return {
    type,
    path: item.path ? String(item.path) : undefined,
    from: item.from ? String(item.from) : undefined,
    to: item.to ? String(item.to) : undefined,
    reason: item.reason ? String(item.reason) : undefined,
  };
}

function buildSortReverseOperations(operations) {
  const source = Array.isArray(operations) ? operations : [];
  const reversed = [];

  for (let i = source.length - 1; i >= 0; i -= 1) {
    const op = normalizeSortOperationForLog(source[i]);
    if (!op || op.type !== 'moveFile' || !op.from || !op.to) {
      continue;
    }

    reversed.push({
      type: 'moveFile',
      from: op.to,
      to: op.from,
      reason: `reverse of ${op.from} -> ${op.to}`,
    });
  }

  return reversed;
}

function writeSortChangeLog({ planId, pending, applyResp }) {
  const baseDir = String(pending?.baseDir || '').trim();
  if (!baseDir) {
    throw new Error('缺少 baseDir，无法写入 changelog.json');
  }

  const plan = pending?.plan && typeof pending.plan === 'object' ? pending.plan : {};
  const operations = Array.isArray(plan.operations)
    ? plan.operations.map(normalizeSortOperationForLog).filter(Boolean)
    : [];
  const reverseOperations = buildSortReverseOperations(operations);
  const changelogPath = path.join(baseDir, SORT_CHANGELOG_FILE_NAME);

  const payload = {
    version: 1,
    generatedAt: new Date().toISOString(),
    source: 'sort-skill',
    planId: String(planId || ''),
    baseDir,
    strategy: String(pending?.strategy || ''),
    objective: String(pending?.objective || ''),
    aiDesign: {
      outline: Array.isArray(pending?.aiOutline) ? pending.aiOutline : [],
      notes: Array.isArray(pending?.aiNotes) ? pending.aiNotes : [],
      renameMap: pending?.renameMap && typeof pending.renameMap === 'object' ? pending.renameMap : {},
    },
    summary: applyResp?.summary || plan.summary || null,
    execution: applyResp?.execution || null,
    changes: {
      operations,
      reverseOperations,
      movedCount: reverseOperations.length,
    },
    restoreHint: '可让 AI 读取本文件的 changes.reverseOperations 并按顺序执行，以恢复到上次整理前状态。',
  };

  fs.writeFileSync(changelogPath, `${JSON.stringify(payload, null, 2)}\n`, 'utf8');
  return changelogPath;
}

function parseSortSkillCommand(text) {
  const originalText = String(text || '').trim();
  const commandSegment = extractSortCommandSegment(originalText);
  if (!commandSegment) {
    return null;
  }

  const tokens = tokenizeSkillArgs(commandSegment);
  if (tokens.length === 0) {
    return null;
  }

  const command = String(tokens[0] || '').toLowerCase();
  if (!SORT_SKILL_COMMANDS.has(command)) {
    return null;
  }

  const errors = [];
  const objectiveParts = [];
  let mode = 'plan';
  let planId = '';
  let snapshotId = '';
  let baseDir = '';
  let strategy = 'byAi';
  let maxDepth = 2;
  let skipHidden = true;
  let includeShortcuts = false;
  let focus = '';
  let extensionFilters = [];
  let keepLatest = 0;

  for (let i = 1; i < tokens.length; i += 1) {
    const token = String(tokens[i] || '').trim();
    if (!token) {
      continue;
    }

    const lowered = token.toLowerCase();
    if (lowered === 'apply' || lowered === '执行') {
      mode = 'apply';
      planId = String(tokens[i + 1] || '').trim();
      break;
    }

    if (lowered === 'rollback' || lowered === '回滚') {
      mode = 'rollback';
      snapshotId = String(tokens[i + 1] || '').trim();
      i += 1;
      continue;
    }

    if (lowered === 'history' || lowered === 'snapshots' || lowered === '历史') {
      mode = 'history';
      continue;
    }

    if (lowered === 'storage' || lowered === '仓库信息' || lowered === '空间') {
      mode = 'storage';
      continue;
    }

    if (lowered === 'compact' || lowered === 'gc' || lowered === '压缩') {
      mode = 'compact';
      continue;
    }

    if (lowered === 'pending' || lowered === 'list' || lowered === '计划列表') {
      mode = 'pending';
      break;
    }

    if (lowered === 'help' || lowered === '--help' || lowered === '-h') {
      mode = 'help';
      break;
    }

    if (lowered === '--dir') {
      baseDir = String(tokens[i + 1] || '').trim();
      if (!baseDir) {
        errors.push('参数 --dir 缺少值。');
      }
      i += 1;
      continue;
    }

    if (lowered === '--strategy') {
      const value = String(tokens[i + 1] || '').trim();
      if (!value) {
        errors.push('参数 --strategy 缺少值。');
      } else {
        strategy = normalizeSortStrategy(value);
      }
      i += 1;
      continue;
    }

    if (lowered === '--depth') {
      const value = Number.parseInt(tokens[i + 1], 10);
      if (!Number.isFinite(value)) {
        errors.push('参数 --depth 必须是数字。');
      } else {
        maxDepth = Math.min(Math.max(value, 1), 6);
      }
      i += 1;
      continue;
    }

    if (lowered === '--all') {
      skipHidden = false;
      continue;
    }

    if (lowered === '--with-shortcuts') {
      includeShortcuts = true;
      continue;
    }

    if (lowered === '--focus') {
      const value = String(tokens[i + 1] || '').trim();
      if (!value) {
        errors.push('参数 --focus 缺少值。');
      } else {
        const normalizedFocus = normalizeSortFocus(value);
        if (!normalizedFocus) {
          errors.push(`参数 --focus 不支持: ${value}`);
        } else {
          focus = normalizedFocus;
        }
      }
      i += 1;
      continue;
    }

    if (lowered === '--keep') {
      const value = Number.parseInt(tokens[i + 1], 10);
      if (!Number.isFinite(value)) {
        errors.push('参数 --keep 必须是数字。');
      } else {
        keepLatest = Math.min(Math.max(value, 10), 500);
      }
      i += 1;
      continue;
    }

    if (token.startsWith('--')) {
      errors.push(`未知参数: ${token}`);
      continue;
    }

    objectiveParts.push(token);
  }

  if (!baseDir) {
    baseDir = inferSortBaseDirFromText(objectiveParts.join(' '));
  }

  if (!focus && mode === 'plan' && String(strategy || '').toLowerCase() !== 'byai') {
    focus = inferSortFocusFromObjective(objectiveParts.join(' '));
  }

  if (mode === 'plan' && String(strategy || '').toLowerCase() === 'byai') {
    extensionFilters = inferSortExtensionsFromObjective(objectiveParts.join(' '));
  }

  return {
    invoked: true,
    mode,
    planId,
    snapshotId,
    baseDir,
    strategy,
    maxDepth,
    skipHidden,
    includeShortcuts,
    focus,
    extensionFilters,
    keepLatest,
    objective: objectiveParts.join(' ').trim(),
    errors,
  };
}

function createSortPlanId() {
  return `sort-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 7)}`;
}

function formatPendingSortPlans() {
  if (pendingSortPlans.size === 0) {
    return '当前没有待确认的 /sort 计划。';
  }

  const lines = ['待确认 /sort 计划:'];
  for (const [planId, item] of pendingSortPlans.entries()) {
    const createdText = new Date(item.createdAt || Date.now()).toLocaleString();
    lines.push(`- ${planId} | baseDir=${item.baseDir} | strategy=${item.strategy} | 创建时间=${createdText}`);
  }
  lines.push('确认执行: /sort apply <planId>');
  return lines.join('\n');
}

async function resolveSortSkillResponse(userText, settings) {
  const parsed = parseSortSkillCommand(userText);
  if (!parsed) {
    return null;
  }

  if (parsed.errors.length > 0) {
    return {
      ok: false,
      text: `sort skill 参数错误: ${parsed.errors.join('；')}`,
    };
  }

  if (parsed.mode === 'help') {
    return {
      ok: true,
      text: [
        '用法:',
        '/sort [--dir desktop|documents|downloads|current|<绝对路径>] [--strategy ai|type|project|date] [--depth 1..6] [--all] [--with-shortcuts] [--focus archive|image|document|video|audio|code|project|other|all]',
        '说明: --dir desktop 与不传 --dir 时，优先使用 settings.json 的 workingDirectory。',
        '说明: 默认策略为 ai，由模型决定目录结构与归位计划。',
        '说明: 默认忽略快捷方式(.lnk/.url)，可用 --with-shortcuts 开启。',
        '/sort pending',
        '/sort apply <planId>',
        '/sort history [--dir <路径>]',
        '/sort rollback <snapshotId> [--dir <路径>]',
        '/sort storage [--dir <路径>]',
        '/sort compact [--dir <路径>] [--keep 10..500]',
        '示例: /sort --dir desktop --strategy ai',
        '示例: /sort --dir desktop --strategy type',
        '示例: /sort --dir downloads --strategy date',
        '示例: /sort --dir desktop --focus archive 把桌面压缩包归档',
        '示例: /sort rollback a1b2c3d4e5f6',
      ].join('\n'),
    };
  }

  if (parsed.mode === 'pending') {
    return { ok: true, text: formatPendingSortPlans() };
  }

  const requestedDir = resolveSortRequestedDir(parsed.baseDir, settings) || settings.workingDirectory;

  if (parsed.mode === 'history') {
    const historyResp = await runRustGitConnect({
      action: 'snapshotList',
      workspaceDir: requestedDir,
      limit: 30,
    });

    if (!historyResp?.ok) {
      return {
        ok: false,
        text: `读取快照历史失败: ${String(historyResp?.text || '未知错误')}`,
      };
    }

    const snapshots = Array.isArray(historyResp.snapshots) ? historyResp.snapshots : [];
    if (snapshots.length === 0) {
      return {
        ok: true,
        text: [
          `当前目录暂无快照: ${requestedDir}`,
          '先执行 /sort apply 生成快照后，再使用 /sort rollback <snapshotId> 回滚。',
        ].join('\n'),
      };
    }

    const lines = [
      `快照历史 (${requestedDir})，最近 ${snapshots.length} 条:`,
      ...snapshots.map((item) => {
        const id = String(item?.snapshotId || '').slice(0, 12);
        const status = String(item?.status || 'ok');
        const createdAt = String(item?.createdAt || '');
        const message = String(item?.message || '').replace(/\r?\n/g, ' ').trim();
        return `- ${id} | status=${status} | time=${createdAt} | ${message}`;
      }),
      '回滚命令: /sort rollback <snapshotId>',
    ];

    return { ok: true, text: lines.join('\n') };
  }

  if (parsed.mode === 'storage') {
    const keepLatest = Number(settings.gitSnapshotKeepLatest || DEFAULT_GIT_SNAPSHOT_KEEP_LATEST);
    const storageResp = await runRustGitConnect({
      action: 'storageInfo',
      workspaceDir: requestedDir,
      keepLatest,
    });

    if (!storageResp?.ok) {
      return {
        ok: false,
        text: `读取 Git 存储信息失败: ${String(storageResp?.text || '未知错误')}`,
      };
    }

    const storage = storageResp.storage && typeof storageResp.storage === 'object'
      ? storageResp.storage
      : null;

    return {
      ok: true,
      text: [
        `Git 仓库: ${String(storageResp.repoPath || '')}`,
        storage ? `存储占用: ${String(storage.human || '')} (${Number(storage.bytes || 0)} bytes)` : '',
        storage ? `快照引用: ${Number(storage.snapshotRefs || 0)}` : '',
        storage ? `保留策略: 最近 ${Number(storage.keepLatest || keepLatest)} 个快照` : '',
        '可执行压缩: /sort compact',
      ].filter(Boolean).join('\n'),
    };
  }

  if (parsed.mode === 'compact') {
    const fallbackKeep = Number(settings.gitSnapshotKeepLatest || DEFAULT_GIT_SNAPSHOT_KEEP_LATEST);
    const keepLatest = parsed.keepLatest > 0 ? parsed.keepLatest : fallbackKeep;
    const compactResp = await runRustGitConnect({
      action: 'compactStorage',
      workspaceDir: requestedDir,
      keepLatest,
    });

    if (!compactResp?.ok) {
      return {
        ok: false,
        text: `Git 压缩失败: ${String(compactResp?.text || '未知错误')}`,
      };
    }

    const storage = compactResp.storage && typeof compactResp.storage === 'object'
      ? compactResp.storage
      : null;

    return {
      ok: true,
      text: [
        String(compactResp.text || 'Git 压缩完成。'),
        storage ? `压缩后占用: ${String(storage.human || '')} (${Number(storage.bytes || 0)} bytes)` : '',
        storage ? `快照引用: ${Number(storage.snapshotRefs || 0)}` : '',
      ].filter(Boolean).join('\n'),
    };
  }

  if (parsed.mode === 'rollback') {
    const snapshotId = String(parsed.snapshotId || '').trim();
    if (!snapshotId) {
      return {
        ok: false,
        text: 'rollback 模式缺少 snapshotId。请使用 /sort rollback <snapshotId>，或先 /sort history 查看历史。',
      };
    }

    const rollbackResp = await runRustGitConnect({
      action: 'rollback',
      workspaceDir: requestedDir,
      snapshotId,
    });

    if (!rollbackResp?.ok) {
      return {
        ok: false,
        text: `回滚失败: ${String(rollbackResp?.text || '未知错误')}`,
      };
    }

    return {
      ok: true,
      text: [
        String(rollbackResp.text || '回滚成功。'),
        `目录: ${requestedDir}`,
        `快照: ${snapshotId}`,
      ].join('\n'),
    };
  }

  if (parsed.mode === 'apply') {
    let resolvedPlanId = String(parsed.planId || '').trim();
    if (!resolvedPlanId) {
      if (pendingSortPlans.size === 1) {
        [resolvedPlanId] = Array.from(pendingSortPlans.keys());
      } else {
        return {
          ok: false,
          text: 'apply 模式缺少 planId。请使用 /sort apply <planId>，或先 /sort pending 查看待执行计划。',
        };
      }
    }

    const pending = pendingSortPlans.get(resolvedPlanId);
    if (!pending) {
      return {
        ok: false,
        text: `未找到计划 ${resolvedPlanId}。请先 /sort pending 查看可执行计划。`,
      };
    }

    const snapshotResp = await runRustGitConnect({
      action: 'snapshotCreate',
      workspaceDir: pending.baseDir,
      operationName: 'sort.apply',
      metadata: {
        source: 'sort-skill',
        planId: resolvedPlanId,
        strategy: String(pending.strategy || ''),
        objective: String(pending.objective || ''),
        createdAt: new Date().toISOString(),
      },
    });

    if (!snapshotResp?.ok) {
      return {
        ok: false,
        text: `创建执行前快照失败，已中止 apply: ${String(snapshotResp?.text || '未知错误')}`,
      };
    }

    const snapshotId = String(snapshotResp.snapshotId || '').trim();

    const applyResp = await runRustSort({
      action: 'apply',
      baseDir: pending.baseDir,
      strategy: pending.strategy,
      dryRun: false,
      plan: pending.plan,
    });

    if (!applyResp?.ok) {
      if (snapshotId) {
        try {
          await runRustGitConnect({
            action: 'snapshotMarkFailed',
            workspaceDir: pending.baseDir,
            snapshotId,
            reason: String(applyResp?.text || 'sort apply failed'),
          });
        } catch {
          // Keep original apply error as the primary failure.
        }
      }
      return {
        ok: false,
        text: [
          `排序执行失败: ${String(applyResp?.text || '未知错误')}`,
          snapshotId ? `已将快照标记为失败: ${snapshotId}` : '',
        ].filter(Boolean).join('\n'),
      };
    }

    let changelogPath = '';
    let changelogWarning = '';
    try {
      changelogPath = writeSortChangeLog({
        planId: resolvedPlanId,
        pending,
        applyResp,
      });
    } catch (error) {
      changelogWarning = `changelog.json 写入失败: ${error?.message || String(error)}`;
    }

    pendingSortPlans.delete(resolvedPlanId);
    const lines = [
      String(applyResp.text || '排序执行成功。'),
      `计划ID: ${resolvedPlanId}`,
      snapshotId ? `快照ID: ${snapshotId}` : '',
      applyResp.execution
        ? `执行统计: moved=${Number(applyResp.execution.moved || 0)} createdDirs=${Number(applyResp.execution.createdDirs || 0)} skipped=${Number(applyResp.execution.skipped || 0)} failed=${Number(applyResp.execution.failed || 0)}`
        : '',
      changelogPath ? `变更记录: ${changelogPath}` : '',
      changelogWarning,
      snapshotId ? `回滚命令: /sort rollback ${snapshotId}` : '',
      '提示: /sort 技能不会删除文件，只会移动到分类目录。',
    ].filter(Boolean);

    return { ok: true, text: lines.join('\n') };
  }

  const preferAiPlanning = String(parsed.strategy || '').toLowerCase() === 'byai';

  const planResp = await runRustSort({
    action: 'plan',
    baseDir: requestedDir,
    strategy: parsed.strategy,
    skipHidden: parsed.skipHidden,
    includeShortcuts: parsed.includeShortcuts,
    maxDepth: parsed.maxDepth,
    includeEntries: preferAiPlanning,
  });

  if (!planResp?.ok) {
    return {
      ok: false,
      text: `排序计划生成失败: ${String(planResp?.text || '未知错误')}`,
    };
  }

  if (preferAiPlanning) {
    const baseDir = String(planResp.baseDir || requestedDir);
    const focusedEntries = filterSortEntriesByFocus(planResp.entries, parsed.focus);
    const scopedByExtensions = filterSortEntriesByExtensions(focusedEntries.entries, parsed.extensionFilters);
    const candidateEntries = scopedByExtensions.entries;
    const focusCategory = focusedEntries.focus;
    const focusDisplay = getSortFocusDisplayName(focusCategory);
    const extensionLabel = scopedByExtensions.extensions.length > 0
      ? scopedByExtensions.extensions.map((ext) => `.${ext}`).join(', ')
      : '';

    if (candidateEntries.length === 0) {
      const noOpLines = [
        String(planResp.text || '计划生成完成。'),
        extensionLabel
          ? `已按文件类型过滤到 ${extensionLabel}，但当前目录没有可执行的对应文件。`
          : (focusCategory ? `已按你的目标聚焦到“${focusDisplay}”，但当前目录没有可执行的对应文件。` : '当前没有可执行操作。'),
        parsed.includeShortcuts ? '本次已包含快捷方式。' : '默认已忽略快捷方式（.lnk/.url 等）。',
      ].filter(Boolean);
      return {
        ok: true,
        text: noOpLines.join('\n'),
      };
    }

    let aiOutline = [];
    let aiNotes = [];
    let aiFolders = [];
    let aiFallbackReason = '';
    let effectivePlan = null;

    try {
      const aiPrompt = buildSortAiGroupingPrompt({
        userText,
        objective: parsed.objective,
        requestedDir: baseDir,
        focus: focusCategory,
        includeShortcuts: parsed.includeShortcuts,
        entries: candidateEntries,
      });

      const aiText = await runClaudePrompt(aiPrompt, [], false, settings, true);
      const parsedDesign = parseSortGroupingFromAiText(aiText);
      if (parsedDesign.ok) {
        aiOutline = parsedDesign.outline;
        aiNotes = parsedDesign.notes;
        const built = buildSortPlanFromAiGroups({
          baseDir,
          entries: candidateEntries,
          groups: parsedDesign.groups,
        });

        const moveOps = Array.isArray(built?.plan?.operations)
          ? built.plan.operations.filter((op) => String(op?.type || '') === 'moveFile').length
          : 0;
        if (moveOps > 0) {
          effectivePlan = built.plan;
          aiFolders = built.folders;
        } else {
          aiFallbackReason = 'AI 未生成可执行移动操作，已回退到规则分类。';
        }
      } else {
        aiFallbackReason = 'AI 输出格式不符合要求，已回退到规则分类。';
      }
    } catch {
      aiFallbackReason = 'AI 规划调用失败，已回退到规则分类。';
    }

    if (!effectivePlan) {
      if (candidateEntries.length > 0 && shouldUseSingleFolderFallback(parsed.objective, scopedByExtensions.extensions)) {
        const folderName = inferSortSingleFolderName(parsed.objective, scopedByExtensions.extensions);
        const built = buildSortPlanFromAiGroups({
          baseDir,
          entries: candidateEntries,
          groups: [
            {
              name: folderName,
              files: candidateEntries.map((entry) => entry.relativePath),
            },
          ],
        });

        const moveOps = Array.isArray(built?.plan?.operations)
          ? built.plan.operations.filter((op) => String(op?.type || '') === 'moveFile').length
          : 0;
        if (moveOps > 0) {
          effectivePlan = built.plan;
          aiFolders = built.folders;
          aiFallbackReason = `AI 输出格式不符合要求，已按${extensionLabel || '目标范围'}回退到单目录方案。`;
        }
      }

      if (!effectivePlan) {
      const sourcePlan = planResp.plan;
      const focused = filterSortPlanByFocus(sourcePlan, focusCategory);
      effectivePlan = focused.plan;
      if (focused.removedMoves > 0) {
        aiNotes = [...aiNotes, `已自动排除 ${focused.removedMoves} 条与目标无关的移动操作。`];
      }
      }
    }

    const ops = Array.isArray(effectivePlan?.operations) ? effectivePlan.operations : [];
    if (!effectivePlan || ops.length === 0) {
      return {
        ok: true,
        text: [
          String(planResp.text || '计划生成完成。'),
          aiFallbackReason,
          '当前没有可执行操作。',
        ].filter(Boolean).join('\n'),
      };
    }

    const effectivePreview = buildSortPreviewFromPlan(effectivePlan, 50);
    const planId = createSortPlanId();
    pendingSortPlans.set(planId, {
      plan: effectivePlan,
      baseDir,
      strategy: String(planResp.strategy || parsed.strategy || 'byAi'),
      objective: String(parsed.objective || ''),
      focus: focusCategory,
      includeShortcuts: parsed.includeShortcuts,
      aiOutline,
      aiNotes,
      aiFolders,
      renameMap: {},
      sourceUserText: String(userText || '').trim(),
      createdAt: Date.now(),
    });

    const preview = Array.isArray(effectivePreview)
      ? effectivePreview.slice(0, 50)
      : [];
    const folderLines = aiFolders.slice(0, 12).map((item) => `- ${item.name}: ${item.count} 个文件`);
    const lines = [
      String(planResp.text || '计划生成完成。'),
      extensionLabel
        ? `已按文件类型过滤: ${extensionLabel}。`
        : (focusCategory ? `已按你的目标聚焦到“${focusDisplay}”。` : '已按全量文件生成整理计划。'),
      focusedEntries.removed > 0 ? `按聚焦条件过滤了 ${focusedEntries.removed} 个文件。` : '',
      scopedByExtensions.removed > 0 ? `按扩展名过滤了 ${scopedByExtensions.removed} 个文件。` : '',
      parsed.includeShortcuts ? '本次会处理快捷方式。' : '本次默认已忽略快捷方式（.lnk/.url 等）。',
      aiFallbackReason || '已由 AI 生成自定义分类目录与文件归位计划。',
      aiOutline.length > 0 ? 'AI整理大纲:' : '',
      ...aiOutline.map((v, i) => `${i + 1}. ${v}`),
      folderLines.length > 0 ? 'AI目录分配:' : '',
      ...folderLines,
      aiNotes.length > 0 ? 'AI备注:' : '',
      ...aiNotes.map((v) => `- ${v}`),
      `计划ID: ${planId}`,
      `目标目录: ${baseDir}`,
      `策略: ${String(planResp.strategy || parsed.strategy || 'byAi')}`,
      effectivePlan?.summary
        ? `统计: files=${Number(effectivePlan.summary.totalFiles || 0)} moves=${Number(effectivePlan.summary.plannedMoves || 0)} skipped=${Number(effectivePlan.summary.skipped || 0)}`
        : '',
      preview.length > 0 ? '预览:' : '',
      ...preview.map((line) => `- ${line}`),
      '',
      `如同意执行，请发送: /sort apply ${planId}`,
      '如需查看待执行计划，请发送: /sort pending',
    ].filter(Boolean);

    return { ok: true, text: lines.join('\n') };
  }

  const sourcePlan = planResp.plan;
  const focused = filterSortPlanByFocus(sourcePlan, parsed.focus);
  const focusCategory = focused.focus;
  const focusDisplay = getSortFocusDisplayName(focusCategory);
  const plan = focused.plan;
  const ops = Array.isArray(plan?.operations) ? plan.operations : [];
  if (!plan || ops.length === 0) {
    const noOpLines = [
      String(planResp.text || '计划生成完成'),
      focusCategory ? `已按你的目标聚焦到“${focusDisplay}”，但当前目录没有可执行的对应文件。` : '当前没有可执行操作。',
      parsed.includeShortcuts ? '本次已包含快捷方式。' : '默认已忽略快捷方式（.lnk/.url 等）。',
    ].filter(Boolean);
    return {
      ok: true,
      text: noOpLines.join('\n'),
    };
  }

  let aiOutline = [];
  let aiNotes = [];
  let renameMap = {};
  try {
    const aiPrompt = buildSortAiPrompt({
      userText,
      objective: parsed.objective,
      requestedDir,
      strategy: parsed.strategy,
      summary: plan.summary,
      preview: buildSortPreviewFromPlan(plan, 25),
      focus: focusCategory,
      includeShortcuts: parsed.includeShortcuts,
    });
    const aiText = await runClaudePrompt(aiPrompt, [], false, settings, true);
    const parsedDesign = parseSortDesignFromAiText(aiText);
    if (parsedDesign.ok) {
      aiOutline = parsedDesign.outline;
      aiNotes = parsedDesign.notes;
      renameMap = parsedDesign.renameMap;
    }
  } catch {
    // Keep default non-AI plan as safe fallback.
  }

  const effectivePlan = applySortFolderRenamesToPlan(plan, String(planResp.baseDir || requestedDir), renameMap);
  const effectivePreview = buildSortPreviewFromPlan(effectivePlan, 50);

  const planId = createSortPlanId();
  pendingSortPlans.set(planId, {
    plan: effectivePlan,
    baseDir: String(planResp.baseDir || requestedDir),
    strategy: String(planResp.strategy || parsed.strategy || 'byType'),
    objective: String(parsed.objective || ''),
    focus: focusCategory,
    includeShortcuts: parsed.includeShortcuts,
    aiOutline,
    aiNotes,
    renameMap,
    sourceUserText: String(userText || '').trim(),
    createdAt: Date.now(),
  });

  const preview = Array.isArray(effectivePreview)
    ? effectivePreview.slice(0, 50)
    : [];

  const renameLines = Object.entries(renameMap).map(([from, to]) => `- ${from} -> ${to}`);

  const lines = [
    String(planResp.text || '计划生成完成。'),
    focusCategory ? `已按你的目标聚焦到“${focusDisplay}”。` : '已按全量文件生成整理计划。',
    parsed.includeShortcuts ? '本次会处理快捷方式。' : '本次默认已忽略快捷方式（.lnk/.url 等）。',
    focused.removedMoves > 0 ? `已自动排除 ${focused.removedMoves} 条与目标无关的移动操作。` : '',
    aiOutline.length > 0 ? 'AI整理大纲:' : '',
    ...aiOutline.map((v, i) => `${i + 1}. ${v}`),
    renameLines.length > 0 ? 'AI分类命名:' : '',
    ...renameLines,
    aiNotes.length > 0 ? 'AI备注:' : '',
    ...aiNotes.map((v) => `- ${v}`),
    `计划ID: ${planId}`,
    `目标目录: ${String(planResp.baseDir || requestedDir)}`,
    `策略: ${String(planResp.strategy || parsed.strategy || 'byType')}`,
    plan?.summary
      ? `统计: files=${Number(plan.summary.totalFiles || 0)} moves=${Number(plan.summary.plannedMoves || 0)} skipped=${Number(plan.summary.skipped || 0)}`
      : '',
    preview.length > 0 ? '预览:' : '',
    ...preview.map((line) => `- ${line}`),
    '',
    `如同意执行，请发送: /sort apply ${planId}`,
    '如需查看待执行计划，请发送: /sort pending',
  ].filter(Boolean);

  return { ok: true, text: lines.join('\n') };
}

function sanitizeRuntimeSettings(rawSettings) {
  const workspaceRoot = resolveWorkspaceRoot();
  const input = rawSettings && typeof rawSettings === 'object' ? rawSettings : {};

  const toResolvedDir = (value) => {
    const trimmed = String(value || '').trim();
    if (!trimmed) {
      return '';
    }

    const resolved = path.isAbsolute(trimmed) ? trimmed : path.resolve(workspaceRoot, trimmed);
    try {
      if (fs.existsSync(resolved) && fs.statSync(resolved).isDirectory()) {
        return resolved;
      }
    } catch {
      // Keep empty fallback below.
    }
    return '';
  };

  const workingDirectory = toResolvedDir(input.workingDirectory) || DEFAULT_SETTINGS.workingDirectory;
  const systemPrompt = String(input.systemPrompt || '').trim() || DEFAULT_SETTINGS.systemPrompt;

  const rawTemperature = Number(input.thinkingTemperature);
  const thinkingTemperature = Number.isFinite(rawTemperature)
    ? Math.min(Math.max(rawTemperature, 0), 1)
    : DEFAULT_SETTINGS.thinkingTemperature;

  const rawInterval = Number(input.thinkingIntervalMs);
  const thinkingIntervalMs = Number.isFinite(rawInterval)
    ? Math.min(Math.max(Math.round(rawInterval), 600), 10000)
    : DEFAULT_SETTINGS.thinkingIntervalMs;

  const rawKeepLatest = Number(input.gitSnapshotKeepLatest);
  const gitSnapshotKeepLatest = Number.isFinite(rawKeepLatest)
    ? Math.min(Math.max(Math.round(rawKeepLatest), 10), 500)
    : DEFAULT_SETTINGS.gitSnapshotKeepLatest;

  const attachmentDirs = [];
  const seen = new Set();
  for (const dir of Array.isArray(input.attachmentDirectories) ? input.attachmentDirectories : []) {
    const resolved = toResolvedDir(dir);
    if (!resolved || seen.has(resolved)) {
      continue;
    }
    seen.add(resolved);
    attachmentDirs.push(resolved);
  }

  const uiTexts = sanitizeUiTexts(input.uiTexts);

  return {
    workingDirectory,
    systemPrompt,
    thinkingTemperature,
    thinkingIntervalMs,
    gitSnapshotKeepLatest,
    attachmentDirectories: attachmentDirs,
    uiTexts,
  };
}

async function ensureGitWorkspaceInitialized(settings) {
  const workspaceDir = String(settings?.workingDirectory || '').trim();
  if (!workspaceDir) {
    return;
  }

  try {
    await runRustGitConnect({
      action: 'init',
      workspaceDir,
      keepLatest: Number(settings?.gitSnapshotKeepLatest || DEFAULT_GIT_SNAPSHOT_KEEP_LATEST),
    });
  } catch (error) {
    // Keep startup non-blocking while still surfacing failures in terminal logs.
    console.warn(`GitConnect 初始化失败: ${error?.message || String(error)}`);
  }
}

async function ensureRuntimeSettingsLoaded(forceReload = false) {
  if (!settingsLoadPromise || forceReload) {
    settingsLoadPromise = (async () => {
      try {
        const response = await runRustJsonBin('ccsettings');
        runtimeSettings = sanitizeRuntimeSettings(response?.settings);
        runtimeSettingsSource = String(response?.source || 'defaults');
        runtimeSettingsWarning = String(response?.warning || '').trim();
      } catch (error) {
        runtimeSettings = { ...DEFAULT_SETTINGS, uiTexts: { ...DEFAULT_UI_TEXTS } };
        runtimeSettingsSource = 'defaults';
        runtimeSettingsWarning = `读取设置失败: ${error?.message || String(error)}`;
      }

      return runtimeSettings;
    })();
  }

  return settingsLoadPromise;
}

function inferMimeType(filePath) {
  const ext = path.extname(filePath).toLowerCase();
  if (ext === '.png') return 'image/png';
  if (ext === '.jpg' || ext === '.jpeg') return 'image/jpeg';
  if (ext === '.webp') return 'image/webp';
  if (ext === '.gif') return 'image/gif';
  if (ext === '.bmp') return 'image/bmp';
  if (ext === '.svg') return 'image/svg+xml';
  if (ext === '.txt' || ext === '.md') return 'text/plain';
  if (ext === '.json') return 'application/json';
  if (ext === '.pdf') return 'application/pdf';
  return 'application/octet-stream';
}

function sanitizeFileName(fileName) {
  const base = String(fileName || 'pasted-file').trim();
  return base.replace(/[<>:"/\\|?*\x00-\x1F]/g, '_');
}

function ensureUploadDir() {
  const dir = path.join(app.getPath('temp'), UPLOAD_DIR_NAME);
  fs.mkdirSync(dir, { recursive: true });
  return dir;
}

function buildAttachmentFromPath(filePath, source = 'picked') {
  if (!filePath || !fs.existsSync(filePath)) {
    return null;
  }

  const stat = fs.statSync(filePath);
  if (!stat.isFile()) {
    return null;
  }

  return {
    path: filePath,
    name: path.basename(filePath),
    size: stat.size,
    mimeType: inferMimeType(filePath),
    source,
  };
}

function saveBufferAsUpload(buffer, originalName, mimeType, source = 'pasted') {
  const uploadDir = ensureUploadDir();
  const ext = path.extname(originalName || '');
  const safeName = sanitizeFileName(originalName || `paste-${Date.now()}${ext || ''}`);
  const uniquePrefix = `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
  const filePath = path.join(uploadDir, `${uniquePrefix}-${safeName}`);
  fs.writeFileSync(filePath, buffer);

  const stat = fs.statSync(filePath);
  return {
    path: filePath,
    name: path.basename(filePath),
    size: stat.size,
    mimeType: mimeType || inferMimeType(filePath),
    source,
  };
}

function normalizeAttachments(rawList) {
  const list = Array.isArray(rawList) ? rawList : [];
  const unique = new Map();

  for (const item of list) {
    const filePath = String(item?.path || '').trim();
    if (!filePath || unique.has(filePath)) {
      continue;
    }

    const normalized = buildAttachmentFromPath(filePath, String(item?.source || 'picked'));
    if (normalized) {
      unique.set(filePath, normalized);
    }
  }

  return Array.from(unique.values());
}

function buildAttachmentPromptLines(attachments) {
  if (!attachments || attachments.length === 0) {
    return [];
  }

  const lines = ['', '以下是用户本条消息附带的文件（可读取路径）：'];
  for (const file of attachments) {
    const isImage = String(file.mimeType || '').startsWith('image/');
    const kind = isImage ? '图片' : '文件';
    lines.push(`- [${kind}] ${file.name}: ${file.path}`);
  }
  lines.push('请结合这些附件内容进行回答；若包含图片，请先描述关键视觉信息再作答。');
  return lines;
}

function formatClaudeOutput({ code, stdout, stderr }, settings = DEFAULT_SETTINGS) {
  if (code === 0) {
    const out = (stdout || '').trim();
    if (out) {
      try {
        const obj = JSON.parse(out);
        if (obj && typeof obj.result === 'string' && obj.result.trim()) {
          return clampRenderText(obj.result.trim());
        }
      } catch {
        // Keep plain text fallback.
      }
      return clampRenderText(out);
    }

    const err = (stderr || '').trim();
    return err ? `Claude 返回空 stdout，stderr: ${err}` : getUiText(settings, 'claudeNoOutput', '(Claude 无输出)');
  }

  const err = (stderr || '').trim();
  return err ? `Claude 调用失败: ${err}` : `Claude 调用失败，退出码: ${code}`;
}

async function runClaudePrompt(
  prompt,
  attachmentDirs = [],
  allowFileEdits = false,
  settings = DEFAULT_SETTINGS,
  disableTools = false,
) {
  const resp = await runRustCcConnect({
    prompt,
    attachment_dirs: attachmentDirs,
    allow_file_edits: Boolean(allowFileEdits),
    disable_tools: Boolean(disableTools),
    working_dir: settings.workingDirectory,
    system_prompt: settings.systemPrompt,
    thinking_temperature: settings.thinkingTemperature,
  });

  if (!resp || resp.ok === false) {
    return `Claude 调用失败: ${resp?.text || 'Rust 后端返回失败'}`;
  }

  return String(resp.text || getUiText(settings, 'claudeNoOutput', '(Claude 无输出)'));
}

ipcMain.handle('chat:send', async (_event, payload) => {
  const settings = await ensureRuntimeSettingsLoaded();
  const text = String(payload?.text || '').trim();
  const context = Array.isArray(payload?.context) ? payload.context : [];
  const attachments = normalizeAttachments(payload?.attachments);
  const allowFileEdits = Boolean(payload?.allowFileEdits);

  if (!text) {
    return { ok: false, text: getUiText(settings, 'emptyInputPrompt', '请输入内容。') };
  }

  const sortSkillResp = await resolveSortSkillResponse(text, settings);
  if (sortSkillResp) {
    return {
      ok: Boolean(sortSkillResp.ok),
      text: String(sortSkillResp.text || ''),
    };
  }

  const skillContext = await resolveEverythingSkillContext(text);

  const lines = ['以下是最近对话上下文（按时间顺序）：'];
  for (const item of context.slice(-12)) {
    const role = item?.role === 'user' ? '用户' : '助手';
    const msg = String(item?.text || '').replace(/\r?\n/g, ' ').trim().slice(0, 220);
    if (msg) {
      lines.push(`${role}: ${msg}`);
    }
  }
  lines.push(...buildAttachmentPromptLines(attachments));
  lines.push(...buildEverythingSkillPromptLines(skillContext));

  const normalizedUserText = text.replace(/\r?\n/g, ' ').trim();
  const skillFollowUp = String(skillContext?.followUp || '').trim();

  lines.push('', '请基于以上上下文，继续回答用户最后这条消息：');
  if (skillContext?.invoked && skillFollowUp) {
    lines.push(skillFollowUp);
  } else if (skillContext?.invoked) {
    lines.push(`用户触发了 everything.search skill，原始指令为: ${normalizedUserText}`);
    lines.push('请先给出最相关结果，再解释你筛选这些结果的依据。若 skill 失败，给出修复步骤。');
  } else {
    lines.push(normalizedUserText);
  }

  const attachmentDirSet = new Set(settings.attachmentDirectories || []);
  for (const file of attachments) {
    const dir = path.dirname(file.path);
    if (dir) {
      attachmentDirSet.add(dir);
    }
  }

  const answer = await runClaudePrompt(
    lines.join('\n'),
    Array.from(attachmentDirSet),
    allowFileEdits,
    settings,
  );
  return { ok: true, text: answer };
});

ipcMain.handle('chat:toggle', () => ({ visible: toggleChatWindow() }));

ipcMain.handle('everything:search', async (_event, payload) => {
  const settings = await ensureRuntimeSettingsLoaded();
  const request = payload && typeof payload === 'object' ? payload : {};

  try {
    const response = await runRustEverythingSearch(request);
    if (!response || typeof response !== 'object') {
      return { ok: false, text: 'Everything 查询返回格式无效。' };
    }
    return response;
  } catch (error) {
    return {
      ok: false,
      query: String(request.query || ''),
      scope: String(request.scope || 'file'),
      endpoint: '',
      returned: 0,
      total: null,
      results: [],
      text: `${getUiText(settings, 'invokeFailedPrefix', '调用失败: ')}${error?.message || String(error)}`,
    };
  }
});

ipcMain.handle('settings:get', async () => {
  const settings = await ensureRuntimeSettingsLoaded();
  return {
    ok: true,
    settings,
    source: runtimeSettingsSource,
    warning: runtimeSettingsWarning || undefined,
  };
});

ipcMain.handle('progress:next', () => ({ text: pickProgressMessage() }));
ipcMain.handle('progress:interval', async () => {
  const settings = await ensureRuntimeSettingsLoaded();
  return { ms: settings.thinkingIntervalMs };
});

ipcMain.handle('files:pick', async () => {
  const settings = await ensureRuntimeSettingsLoaded();
  const result = await dialog.showOpenDialog(chatWindow || petWindow || undefined, {
    title: getUiText(settings, 'pickFileDialogTitle', '选择文件'),
    properties: ['openFile', 'multiSelections'],
    filters: [
      { name: getUiText(settings, 'pickFileFilterSupported', '支持的多模态文件'), extensions: ['png', 'jpg', 'jpeg', 'webp', 'gif', 'bmp', 'svg', 'pdf', 'txt', 'md', 'json'] },
      { name: getUiText(settings, 'pickFileFilterAll', '所有文件'), extensions: ['*'] },
    ],
  });

  if (result.canceled) {
    return { files: [] };
  }

  const files = result.filePaths
    .map((filePath) => buildAttachmentFromPath(filePath, 'picked'))
    .filter(Boolean);
  return { files };
});

ipcMain.handle('files:savePastedDataUrl', (_event, payload) => {
  const dataUrl = String(payload?.dataUrl || '');
  const fileName = String(payload?.fileName || `pasted-${Date.now()}.png`);

  const match = dataUrl.match(/^data:([^;,]+);base64,(.+)$/);
  if (!match) {
    return { ok: false, error: getUiText(runtimeSettings, 'invalidImageData', '无效的图片数据。') };
  }

  try {
    const mimeType = match[1];
    const base64 = match[2];
    const buffer = Buffer.from(base64, 'base64');
    const file = saveBufferAsUpload(buffer, fileName, mimeType, 'pasted-image');
    return { ok: true, file };
  } catch (error) {
    return { ok: false, error: error.message || String(error) };
  }
});

ipcMain.handle('files:savePastedBlob', (_event, payload) => {
  try {
    const base64 = String(payload?.base64 || '');
    const fileName = String(payload?.fileName || `pasted-${Date.now()}`);
    const mimeType = String(payload?.mimeType || 'application/octet-stream');
    if (!base64) {
      return { ok: false, error: getUiText(runtimeSettings, 'emptyFileData', '空文件数据。') };
    }

    const buffer = Buffer.from(base64, 'base64');
    const file = saveBufferAsUpload(buffer, fileName, mimeType, 'pasted-file');
    return { ok: true, file };
  } catch (error) {
    return { ok: false, error: error.message || String(error) };
  }
});

ipcMain.handle('window:close', () => {
  app.quit();
});

ipcMain.on('window:drag', (event, payload) => {
  const win = BrowserWindow.fromWebContents(event.sender);
  if (!win || win.isDestroyed()) {
    return;
  }

  const type = String(payload?.type || '');
  const id = event.sender.id;

  if (type === 'start') {
    const bounds = win.getBounds();
    dragStateByWebContentsId.set(id, {
      startScreenX: Number(payload?.screenX) || 0,
      startScreenY: Number(payload?.screenY) || 0,
      startX: bounds.x,
      startY: bounds.y,
    });
    return;
  }

  if (type === 'move') {
    const state = dragStateByWebContentsId.get(id);
    if (!state) {
      return;
    }

    const screenX = Number(payload?.screenX) || state.startScreenX;
    const screenY = Number(payload?.screenY) || state.startScreenY;
    const desiredX = Math.round(state.startX + (screenX - state.startScreenX));
    const desiredY = Math.round(state.startY + (screenY - state.startScreenY));

    const bounds = win.getBounds();
    const area = getWorkAreaForBounds({ ...bounds, x: desiredX, y: desiredY });
    const next = clampBoundsToArea({ ...bounds, x: desiredX, y: desiredY }, area);
    win.setPosition(next.x, next.y);

    if (petWindow && win.id === petWindow.id && chatWindow && chatWindow.isVisible()) {
      positionChatWindow();
    }
    return;
  }

  if (type === 'end') {
    dragStateByWebContentsId.delete(id);
  }
});

ipcMain.on('window:resize', (event, payload) => {
  const win = BrowserWindow.fromWebContents(event.sender);
  if (!win || win.isDestroyed()) {
    return;
  }

  const type = String(payload?.type || '');
  const id = event.sender.id;

  if (type === 'start') {
    const bounds = win.getBounds();
    resizeStateByWebContentsId.set(id, {
      startScreenX: Number(payload?.screenX) || 0,
      startScreenY: Number(payload?.screenY) || 0,
      startWidth: bounds.width,
      startHeight: bounds.height,
    });
    return;
  }

  if (type === 'move') {
    const state = resizeStateByWebContentsId.get(id);
    if (!state) {
      return;
    }

    const screenX = Number(payload?.screenX) || state.startScreenX;
    const screenY = Number(payload?.screenY) || state.startScreenY;
    const width = Math.max(CHAT_MIN_WINDOW_WIDTH, Math.round(state.startWidth + (screenX - state.startScreenX)));
    const height = Math.max(CHAT_MIN_WINDOW_HEIGHT, Math.round(state.startHeight + (screenY - state.startScreenY)));
    win.setSize(width, height);

    if (chatWindow && win.id === chatWindow.id) {
      positionChatWindow();
    }
    return;
  }

  if (type === 'end') {
    resizeStateByWebContentsId.delete(id);
  }
});

app.whenReady().then(async () => {
  const settings = await ensureRuntimeSettingsLoaded();
  await ensureGitWorkspaceInitialized(settings);

  createPetWindow();
  createChatWindow();
  ensurePetWindowInBounds();
  positionChatWindow();

  screen.on('display-metrics-changed', () => {
    ensurePetWindowInBounds();
    if (chatWindow && chatWindow.isVisible()) {
      positionChatWindow();
    }
  });

  app.on('activate', () => {
    if (!petWindow || petWindow.isDestroyed()) {
      createPetWindow();
    }
    if (!chatWindow || chatWindow.isDestroyed()) {
      createChatWindow();
    }
  });
});

app.on('window-all-closed', () => {
  app.quit();
});
