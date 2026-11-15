/* Static design preview interactions; no network calls, identity storage or telemetry. */
const $ = (selector) => document.querySelector(selector);
const $$ = (selector) => [...document.querySelectorAll(selector)];
$('[data-server-form]')?.addEventListener('submit', (event) => {
  event.preventDefault();
  const url = new URL($('#server-url').value);
  $('[data-server-result]').textContent = ['https:', 'http:'].includes(url.protocol)
    ? `Preview: ${url.host}. No connection was made. The app will discover and confirm this server before switching.`
    : 'Use an HTTP or HTTPS base URL.';
});
$('[data-player-search]')?.addEventListener('submit', (event) => {
  event.preventDefault();
  const query = $('#search').value.trim().toLowerCase();
  const presence = $('#presence').value;
  let count = 0;
  $$('[data-player]').forEach((card) => {
    const show = card.dataset.player.includes(query) && (presence === 'all' || presence === 'online' && card.dataset.presence !== 'offline' || presence === card.dataset.presence);
    card.hidden = !show;
    if (show) count++;
  });
  $('[data-search-count]').textContent = `${count} matching sample service record${count === 1 ? '' : 's'}`;
  $('.filter-empty').style.display = count ? 'none' : 'block';
});
$('[data-event-search]')?.addEventListener('submit', (event) => {
  event.preventDefault();
  const query = $('#stream').value.trim().toLowerCase();
  const type = $('#event-type').value;
  const from = Number($('#from-position').value || 0);
  let count = 0;
  $$('[data-event]').forEach((row) => {
    const show = row.dataset.stream.includes(query) && (type === 'all' || type === row.dataset.event) && Number(row.dataset.position) >= from;
    row.hidden = !show;
    if (show) count++;
  });
  $('[data-event-count]').textContent = `${count} matching sample event${count === 1 ? '' : 's'}`;
  $('[data-event-empty]').style.display = count ? 'none' : 'block';
});
let playbackTimer;
const stopPlayback = () => {
  window.clearInterval(playbackTimer);
  playbackTimer = undefined;
  if ($('[data-play]')) { $('[data-play]').textContent = '▶ Play replay'; $('[data-play]').setAttribute('aria-label', 'Play replay preview'); }
};
const setMode = (mode) => {
  $$('[data-mode]').forEach((button) => button.setAttribute('aria-pressed', String(button.dataset.mode === mode)));
  $('[data-frame-label]').textContent = mode === 'live' ? 'LIVE · UP TO 15 HZ' : 'REPLAY · 1 FRAME / SECOND';
  $('[data-replay-note]').textContent = mode === 'live' ? 'Scrub to explore recorded history · 1 frame / second. Static map preview.' : 'Replay control preview. The clock moves; this illustrative map stays fixed.';
};
const updateTime = () => {
  const seconds = Number($('#timeline').value);
  $('[data-time]').textContent = `${String(Math.floor(seconds / 60)).padStart(2, '0')}:${String(seconds % 60).padStart(2, '0')}`;
};
$$('[data-mode]').forEach((button) => button.addEventListener('click', () => {
  stopPlayback();
  setMode(button.dataset.mode);
  if (button.dataset.mode === 'live') { $('#timeline').value = 402; updateTime(); }
}));
$('[data-live]')?.addEventListener('click', () => { stopPlayback(); setMode('live'); $('#timeline').value = 402; updateTime(); });
$('#timeline')?.addEventListener('input', () => { stopPlayback(); setMode('replay'); updateTime(); });
$('[data-play]')?.addEventListener('click', () => {
  if (playbackTimer) { stopPlayback(); return; }
  setMode('replay');
  if (Number($('#timeline').value) >= 402) $('#timeline').value = 0;
  updateTime();
  $('[data-play]').textContent = 'Ⅱ Pause replay';
  $('[data-play]').setAttribute('aria-label', 'Pause replay preview');
  playbackTimer = window.setInterval(() => {
    $('#timeline').value = Number($('#timeline').value) + 1;
    updateTime();
    if (Number($('#timeline').value) >= 402) stopPlayback();
  }, 1000);
});
$('#floor')?.addEventListener('change', () => {
  $('[data-map-level]').textContent = $('#floor').value.toUpperCase() + ' / PREVIEW';
  $('.game-map > svg').style.opacity = $('#floor').selectedIndex ? '.4' : '1';
});
document.addEventListener('keydown', (event) => {
  if (event.key === 'Escape') {
    const open = $('.server[open]');
    if (open) { open.removeAttribute('open'); open.querySelector('summary').focus(); }
  }
});
