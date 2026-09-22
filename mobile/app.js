const status = document.querySelector('#connection-status');
const pairForm = document.querySelector('#pair-form');
const pairCode = document.querySelector('#pair-code');
const answerCard = document.querySelector('#answer-card');
const emptyState = document.querySelector('#empty-state');
const answerContent = document.querySelector('#answer-content');
const answerText = document.querySelector('#answer-text');
const answerTime = document.querySelector('#answer-time');
const transcript = document.querySelector('#transcript');
const listening = document.querySelector('#listening');
let socket;

// The transport will be connected to the Rust daemon in the next step.
// Keeping the UI transport-agnostic lets us use WebSocket locally and HTTPS
// through a relay later without changing the mobile surface.
pairForm.addEventListener('submit', (event) => {
  event.preventDefault();
  if (pairCode.value.replace(/\D/g, '').length < 6) return;
  status.classList.remove('connected');
  status.innerHTML = '<i></i> Connecting…';
  pairCode.blur();
  if (!window.location.host) return;
  socket = new WebSocket(`ws://${window.location.host}/ws`);
  socket.addEventListener('open', () => {
    socket.send(JSON.stringify({ type: 'pair', code: pairCode.value.replace(/\D/g, '') }));
  });
  socket.addEventListener('message', ({ data }) => {
    const event = JSON.parse(data);
    if (event.type === 'connected') {
      status.innerHTML = '<i></i> Connected';
      listening.textContent = 'Listening';
      transcript.textContent = 'Connected to Hyprly. Waiting for the first answer…';
    } else if (event.type === 'answer_start') {
      emptyState.hidden = true;
      answerContent.hidden = false;
      answerCard.classList.remove('empty');
      answerText.textContent = '';
      answerTime.textContent = '';
    } else if (event.type === 'answer_token') {
      answerText.textContent += event.text;
    } else if (event.type === 'answer') {
      answerText.textContent = event.text;
      answerTime.textContent = new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    } else if (event.type === 'transcript') {
      transcript.textContent = event.text;
    } else if (event.type === 'error') {
      status.classList.remove('connected');
      status.innerHTML = '<i></i> Invalid code';
    }
  });
  socket.addEventListener('close', () => {
    status.classList.remove('connected');
    status.innerHTML = '<i></i> Not paired';
    listening.textContent = 'Waiting';
  });
  socket.addEventListener('error', () => {
    status.classList.remove('connected');
    status.innerHTML = '<i></i> Server unavailable';
    listening.textContent = 'Retry';
  });
});

// Development hook for the upcoming WebSocket client.
window.hyprly = {
  showAnswer(text) {
    emptyState.hidden = true;
    answerContent.hidden = false;
    answerCard.classList.remove('empty');
    answerText.textContent = text;
    answerTime.textContent = new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  },
  updateTranscript(text) { transcript.textContent = text; }
};
