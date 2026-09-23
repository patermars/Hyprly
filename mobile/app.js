const DOM = {
  connectionStatus: document.getElementById('connection-status'),
  pairCard: document.getElementById('pair-card'),
  pairForm: document.getElementById('pair-form'),
  pairCode: document.getElementById('pair-code'),
  answerCard: document.getElementById('answer-card'),
  emptyState: document.getElementById('empty-state'),
  answerContent: document.getElementById('answer-content'),
  answerLabel: document.getElementById('answer-label'),
  answerTime: document.getElementById('answer-time'),
  answerType: document.getElementById('answer-type'),
  answerText: document.getElementById('answer-text'),
  answerBullets: document.getElementById('answer-bullets'),
  answerCode: document.getElementById('answer-code'),
  controlBar: document.getElementById('control-bar'),
  btnPause: document.getElementById('btn-pause'),
  btnClear: document.getElementById('btn-clear'),
  pipelineStatus: document.getElementById('pipeline-status'),
  listening: document.getElementById('listening'),
  transcript: document.getElementById('transcript')
};
let socket = null;
let reconnectAttempts = 0;
let reconnectTimer = null;
let paired = false;
let pairedCode = '';
let paused = false;
let streamingAnswer = '';
let lastRenderedStreamedCode = '';
function connect(code) {
  cancelReconnect();
  socket = new WebSocket(`ws://${location.host}/ws`);
  socket.onopen = () => {
    socket.send(JSON.stringify({ type: 'pair', code }));
  };
  socket.onmessage = (e) => {
    try {
      const event = JSON.parse(e.data);
      handleEvent(event);
    } catch (err) {}
  };
  socket.onclose = () => {
    if (paired) {
      paired = false;
      DOM.connectionStatus.textContent = 'Disconnected';
      DOM.connectionStatus.className = 'status-pill';
      scheduleReconnect();
    }
  };
  socket.onerror = () => {
    DOM.connectionStatus.textContent = 'Error';
    DOM.connectionStatus.className = 'status-pill';
  };
}
function scheduleReconnect() {
  const delay = Math.min(1000 * Math.pow(2, reconnectAttempts), 30000);
  reconnectAttempts++;
  reconnectTimer = setTimeout(() => {
    if (pairedCode) connect(pairedCode);
  }, delay);
}
function cancelReconnect() {
  if (reconnectTimer) clearTimeout(reconnectTimer);
  reconnectTimer = null;
}
function handleEvent(event) {
  switch (event.type) {
    case 'connected':
      DOM.connectionStatus.textContent = 'Connected';
      DOM.connectionStatus.className = 'status-pill connected';
      DOM.controlBar.hidden = false;
      paired = true;
      pairedCode = DOM.pairCode.value || pairedCode;
      reconnectAttempts = 0;
      DOM.listening.textContent = 'Listening...';
      DOM.pairCard.hidden = true;
      break;
    case 'status':
      DOM.pipelineStatus.textContent = event.message || event.state;
      DOM.pipelineStatus.className = `pipeline-status ${event.state}`;
      break;
    case 'answer_start':
      DOM.answerCard.classList.remove('empty');
      DOM.emptyState.hidden = true;
      DOM.answerContent.hidden = false;
      streamingAnswer = '';
      lastRenderedStreamedCode = '';
      DOM.answerLabel.textContent = DOM.answerText.textContent.trim() ? 'UPDATING' : 'THINKING';
      DOM.answerTime.textContent = new Date().toLocaleTimeString();
      break;
    case 'answer_token':
      streamingAnswer += event.text;
      const streamedText = readJsonStringField(streamingAnswer, 'answer');
      if (streamedText !== null) {
        DOM.answerText.innerHTML = renderStreamingMarkdown(streamedText);
      }
      const streamedLanguage = readJsonStringField(streamingAnswer, 'language') || '';
      const streamedCode = readJsonStringField(streamingAnswer, 'source');
      if (streamedCode !== null &&
          (lastRenderedStreamedCode === '' ||
           streamedCode.length - lastRenderedStreamedCode.length >= 32 ||
           streamedCode.endsWith('\n'))) {
        renderCode(streamedCode, streamedLanguage);
        lastRenderedStreamedCode = streamedCode;
      }
      break;
    case 'answer':
      DOM.answerText.innerHTML = renderMarkdown(event.text);
      DOM.answerLabel.textContent = 'READY';
      if (event.structured) {
        if (event.structured.type) {
          DOM.answerType.textContent = event.structured.type.replace(/_/g, ' ');
          DOM.answerType.hidden = false;
        }
        if (event.structured.bullets && event.structured.bullets.length) {
          DOM.answerBullets.innerHTML = `<ul>${event.structured.bullets.map(b => `<li>${escapeHTML(b)}</li>`).join('')}</ul>`;
          DOM.answerBullets.hidden = false;
        }
        if (event.structured.code) {
          const { language, source } = event.structured.code;
          renderCode(source, language || '');
        }
      }
      break;
    case 'transcript':
      DOM.transcript.textContent = event.text;
      break;
    case 'topic_reset':
      DOM.answerText.innerHTML = '';
      DOM.answerBullets.innerHTML = '';
      DOM.answerBullets.hidden = true;
      DOM.answerCode.innerHTML = '';
      DOM.answerCode.hidden = true;
      DOM.answerType.textContent = '';
      DOM.answerType.hidden = true;
      DOM.answerCard.classList.add('empty');
      DOM.emptyState.hidden = false;
      DOM.answerContent.hidden = true;
      break;
    case 'error':
      DOM.connectionStatus.textContent = 'Error';
      DOM.connectionStatus.className = 'status-pill';
      break;
  }
}
function escapeHTML(str) {
  return str.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}
function renderMarkdown(text) {
  let html = escapeHTML(text);
  const codeBlocks = [];
  html = html.replace(/```([\w]*)\n([\s\S]*?)```/g, (match, lang, code) => {
    const unescaped = code.replace(/&amp;/g, '&').replace(/&lt;/g, '<').replace(/&gt;/g, '>');
    codeBlocks.push({ lang, code: unescaped });
    return `@@CODE_BLOCK_${codeBlocks.length - 1}@@`;
  });
  html = html.replace(/\*\*([^\*]+)\*\*/g, '<strong>$1</strong>');
  html = html.replace(/__([^_]+)__/g, '<strong>$1</strong>');
  html = html.replace(/\*([^\*]+)\*/g, '<em>$1</em>');
  html = html.replace(/_([^_]+)_/g, '<em>$1</em>');
  html = html.replace(/`([^`]+)`/g, '<code>$1</code>');
  html = html.replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<a href="$2" target="_blank" rel="noopener">$1</a>');
  html = html.replace(/^#### (.*$)/gm, '<h4>$1</h4>');
  html = html.replace(/^### (.*$)/gm, '<h3>$1</h3>');
  html = html.replace(/^## (.*$)/gm, '<h2>$1</h2>');
  html = html.replace(/^# (.*$)/gm, '<h1>$1</h1>');
  let inUl = false, inOl = false;
  const lines = html.split('\n');
  let resultLines = [];
  for (let i = 0; i < lines.length; i++) {
    let line = lines[i];
    if (line.match(/^[\*\-] /)) {
      if (!inUl) { resultLines.push('<ul>'); inUl = true; }
      resultLines.push(`<li>${line.substring(2)}</li>`);
    } else if (line.match(/^\d+\. /)) {
      if (!inOl) { resultLines.push('<ol>'); inOl = true; }
      const txt = line.replace(/^\d+\. /, '');
      resultLines.push(`<li>${txt}</li>`);
    } else {
      if (inUl) { resultLines.push('</ul>'); inUl = false; }
      if (inOl) { resultLines.push('</ol>'); inOl = false; }
      if (line.trim() !== '' && !line.match(/^<h[1-4]>/) && !line.match(/^@@CODE_BLOCK_\d+@@$/)) {
        resultLines.push(`<p>${line}</p>`);
      } else {
        resultLines.push(line);
      }
    }
  }
  if (inUl) resultLines.push('</ul>');
  if (inOl) resultLines.push('</ol>');
  html = resultLines.join('\n');
  for (let i = 0; i < codeBlocks.length; i++) {
    const block = codeBlocks[i];
    const hl = highlight(block.code, block.lang);
    const pre = `<pre><button class="copy-btn">Copy</button><code>${hl}</code></pre>`;
    html = html.replace(`@@CODE_BLOCK_${i}@@`, pre);
  }
  return html;
}
function renderStreamingMarkdown(partial) {
  let toRender = partial;
  const codeBlockCount = (partial.match(/```/g) || []).length;
  if (codeBlockCount % 2 !== 0) {
    const lastIndex = partial.lastIndexOf('```');
    toRender = partial.substring(0, lastIndex);
    const remainder = partial.substring(lastIndex);
    return renderMarkdown(toRender) + '<pre><code>' + escapeHTML(remainder.replace('```', '')) + '</code></pre>';
  }
  return renderMarkdown(toRender);
}
function readJsonStringField(json, field) {
  const marker = `"${field}"`;
  const fieldIndex = json.indexOf(marker);
  if (fieldIndex < 0) return null;

  const colonIndex = json.indexOf(':', fieldIndex + marker.length);
  if (colonIndex < 0) return null;
  let start = colonIndex + 1;
  while (/\s/.test(json[start] || '')) start++;
  if (json[start] !== '"') return null;

  let escaped = false;
  for (let i = start + 1; i < json.length; i++) {
    const char = json[i];
    if (escaped) {
      escaped = false;
    } else if (char === '\\') {
      escaped = true;
    } else if (char === '"') {
      return decodeJsonString(json.slice(start, i + 1));
    }
  }

  // The streamed value is incomplete. Decode the portion received so far;
  // incomplete escape sequences are held back until the next token.
  let partial = json.slice(start);
  if (partial.endsWith('\\')) partial = partial.slice(0, -1);
  return decodeJsonString(`${partial}"`);
}
function decodeJsonString(value) {
  try {
    return JSON.parse(value);
  } catch (_) {
    return null;
  }
}
function renderCode(source, language) {
  const hl = highlight(source, language);
  DOM.answerCode.innerHTML = `<pre><button class="copy-btn">Copy</button><code>${hl}</code></pre>`;
  DOM.answerCode.hidden = false;
  const btn = DOM.answerCode.querySelector('.copy-btn');
  if (btn) {
    btn.onclick = () => {
      navigator.clipboard.writeText(source);
      btn.textContent = 'Copied!';
      btn.classList.add('copied');
      setTimeout(() => {
        btn.textContent = 'Copy';
        btn.classList.remove('copied');
      }, 2000);
    };
  }
}
function highlight(code, lang) {
  let escaped = escapeHTML(code);
  const keywords = 'function const let var return if else for while class import export switch case break try catch async await type interface struct func map chan go defer match impl fn let mut pub use package select from where insert update delete';
  const kwRegex = new RegExp(`\\b(${keywords.split(' ').join('|')})\\b`, 'g');
  escaped = escaped.replace(/(["'`])(?:(?=(\\?))\2.)*?\1/g, '<span class="str">$&</span>');
  escaped = escaped.replace(/\b(\d+(\.\d+)?)\b/g, '<span class="num">$1</span>');
  escaped = escaped.replace(/(\/\/.*|\#.*)/g, '<span class="cm">$&</span>');
  escaped = escaped.replace(/\b([a-zA-Z_]\w*)(?=\s*\()/g, '<span class="fn">$1</span>');
  let temp = escaped.replace(/<span class="[^"]+">.*?<\/span>/g, match => match.replace(kwRegex, m => m));
  const parts = [];
  let lastIdx = 0;
  escaped.replace(/<span class="[^"]+">.*?<\/span>/g, (match, offset) => {
    parts.push(escaped.substring(lastIdx, offset).replace(kwRegex, '<span class="kw">$1</span>'));
    parts.push(match);
    lastIdx = offset + match.length;
  });
  parts.push(escaped.substring(lastIdx).replace(kwRegex, '<span class="kw">$1</span>'));
  return parts.join('');
}
DOM.pairForm.onsubmit = (e) => {
  e.preventDefault();
  const val = DOM.pairCode.value.trim();
  if (val.length === 6) connect(val);
};
DOM.btnPause.onclick = () => {
  paused = !paused;
  DOM.btnPause.textContent = paused ? '▶ Resume' : '⏸ Pause';
  DOM.btnPause.className = paused ? 'active' : '';
  if (socket && socket.readyState === WebSocket.OPEN) {
    socket.send(JSON.stringify({ type: paused ? 'pause' : 'resume' }));
  }
};
DOM.btnClear.onclick = () => {
  DOM.transcript.textContent = 'No audio detected yet...';
  if (socket && socket.readyState === WebSocket.OPEN) {
    socket.send(JSON.stringify({ type: 'clear_transcript' }));
  }
};
document.addEventListener('click', (e) => {
  if (e.target.classList.contains('copy-btn')) {
    const code = e.target.nextElementSibling.textContent;
    navigator.clipboard.writeText(code);
    e.target.textContent = 'Copied!';
    e.target.classList.add('copied');
    setTimeout(() => {
      e.target.textContent = 'Copy';
      e.target.classList.remove('copied');
    }, 2000);
  }
});
window.hyprly = {
  showAnswer: (text) => handleEvent({ type: 'answer', text }),
  updateTranscript: (text) => handleEvent({ type: 'transcript', text })
};
