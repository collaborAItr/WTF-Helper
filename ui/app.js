'use strict';

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);

const CONNECTION_TEXT = {
  connected: 'Connected. Your guide can ask for checks.',
  connecting: 'Connecting…',
  reconnecting: 'Reconnecting…',
  service_off: 'Connected to your account, but WTF Helper is not switched on there yet.',
  not_paired: 'Not connected.',
};

const dateText = (ms) =>
  new Date(ms).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
const timeText = (ms) => new Date(ms).toLocaleTimeString(undefined, { timeStyle: 'short' });

function renderActivity(entries) {
  const log = $('log');
  log.replaceChildren();
  $('no-activity').hidden = entries.length > 0;
  for (const entry of entries) {
    const item = document.createElement('li');
    item.className = entry.ok ? 'ok' : 'refused';
    const when = document.createElement('span');
    when.className = 'when';
    when.textContent = timeText(entry.atMs);
    const what = document.createElement('span');
    what.className = 'what';
    what.textContent = entry.checks.length ? entry.checks.join(', ') : 'A check request';
    const how = document.createElement('span');
    how.className = 'how';
    how.textContent = [entry.approval, entry.outcome].filter(Boolean).join(' · ');
    item.append(when, what, how);
    log.append(item);
  }
}

function render(status) {
  document.title = status.productName;
  $('product').textContent = status.productName;
  $('test-badge').hidden = !status.isTest;
  $('version').textContent = status.version;
  for (const el of document.querySelectorAll('.host')) el.textContent = status.host;

  $('notice').hidden = !status.notice;
  $('notice').textContent = status.notice || '';

  const paired = status.paired;
  $('pair-view').hidden = Boolean(paired);
  $('paired-view').hidden = !paired;
  if (paired) {
    $('connection').textContent = status.paused
      ? 'Paused. Every check is refused.'
      : CONNECTION_TEXT[status.connection] || '';
    $('connection').dataset.state = status.paused ? 'paused' : status.connection;
    $('service').textContent = `${status.serviceName} (${status.host})`;
    $('account').textContent = `Account ID ending ${paired.accountHint}`;
    $('device').textContent = paired.deviceName;
    $('paired-at').textContent = dateText(paired.pairedAtMs);
    $('paused').checked = status.paused;
  } else {
    $('confirm').hidden = true;
    $('disconnect-area').hidden = false;
  }
  renderActivity(status.activity);
}

function showPairError(message) {
  $('pair-error').hidden = !message;
  $('pair-error').textContent = message || '';
}

$('pair-form').addEventListener('submit', async (event) => {
  event.preventDefault();
  const button = $('pair-button');
  button.disabled = true;
  button.textContent = 'Connecting…';
  showPairError('');
  try {
    render(await invoke('pair', { code: $('code').value }));
    $('code').value = '';
  } catch (error) {
    showPairError(String(error));
  } finally {
    button.disabled = false;
    button.textContent = 'Connect';
  }
});

$('paused').addEventListener('change', async (event) => {
  try {
    render(await invoke('set_paused', { paused: event.target.checked }));
  } catch {
    render(await invoke('get_status'));
  }
});

$('disconnect').addEventListener('click', () => {
  $('disconnect-area').hidden = true;
  $('confirm').hidden = false;
  $('confirm-no').focus();
});
$('confirm-no').addEventListener('click', () => {
  $('confirm').hidden = true;
  $('disconnect-area').hidden = false;
});
$('confirm-yes').addEventListener('click', async () => {
  render(await invoke('disconnect'));
});

listen('status', (event) => render(event.payload));
invoke('get_status').then(render);
