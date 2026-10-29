'use strict';

(() => {
  const { invoke } = window.__TAURI__.core;
  const appWindow = window.__TAURI__.window.getCurrentWindow();

  // ------------------------------------------------------------- constants

  const MAX_RETRIES = 5;                          // consecutive failures (while online) before a track is skipped
  const RETRY_DELAYS = [0, 1000, 2500, 5000, 8000]; // ms between recovery attempts
  const SLOW_RETRY_MS = 30000;                    // longest wait between retries when nothing else can be tried
  const OFFLINE_POLL_MS = 3000;                   // how often to retry while there is no internet
  const END_TOLERANCE_MS = 4000;                  // within this of the end = the song really finished
  const STALL_TIMEOUT_S = 12;                     // playing but no progress for this long = stalled stream
  const LOAD_TIMEOUT_S = 25;                      // source set but never started playing
  const RAINBOW_CYCLE_SEC = 60;                   // time for the Rainbow theme to go round the colour wheel
  const ROW_H = 30;                               // track list row height (px)

  // ------------------------------------------------------------- helpers

  const $ = (id) => document.getElementById(id);
  const clamp = (v, lo, hi) => Math.max(lo, Math.min(hi, v));
  const nowS = () => performance.now() / 1000;
  const hex = (rgb) => '#' + rgb.map((v) => v.toString(16).padStart(2, '0')).join('');

  function fmt(seconds) {
    const s = Math.floor(seconds);
    const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), sec = s % 60;
    const mm = String(m).padStart(2, '0'), ss = String(sec).padStart(2, '0');
    return h ? `${h}:${mm}:${ss}` : `${mm}:${ss}`;
  }

  // ------------------------------------------------------------- themes

  const theme = (accent, bg, sidebar) => ({ accent, bg, sidebar_bg: sidebar });

  // Same maths as Qt's QColor.fromHslF, returned as 0-255 integers.
  function hsl(h, s, l) {
    const c = (1 - Math.abs(2 * l - 1)) * s;
    const x = c * (1 - Math.abs(((h * 6) % 2) - 1));
    const m = l - c / 2;
    const [r, g, b] = [[c, x, 0], [x, c, 0], [0, c, x], [0, x, c], [x, 0, c], [c, 0, x]][Math.floor(h * 6) % 6];
    return [r, g, b].map((v) => Math.round((v + m) * 255));
  }

  const rainbowTheme = (h) => theme(hsl(h, 0.9, 0.7), hsl(h, 0.5, 0.055), hsl(h, 0.5, 0.03));

  const THEMES = {
    'Pink':      theme([255, 182, 193], [18, 10, 14], [10, 5, 8]),
    'Blue':      theme([135, 206, 235], [10, 14, 20], [5, 8, 12]),
    'Green':     theme([144, 238, 144], [10, 18, 12], [5, 10, 7]),
    'Black':     theme([220, 220, 220], [12, 12, 12], [6, 6, 6]),
    'Purple':    theme([187, 134, 252], [16, 10, 24], [9, 5, 15]),
    'Deep Blue': theme([77, 124, 255], [6, 10, 26], [3, 5, 16]),
    'White':     theme([255, 255, 255], [14, 14, 16], [7, 7, 9]),
    'Silver':    theme([192, 200, 212], [14, 16, 20], [8, 9, 12]),
    'Red':       theme([255, 99, 110], [22, 8, 10], [13, 4, 6]),
    'Orange':    theme([255, 167, 88], [22, 14, 8], [13, 8, 4]),
    'Yellow':    theme([255, 224, 102], [22, 20, 8], [13, 12, 4]),
    'Teal':      theme([64, 224, 208], [6, 20, 20], [3, 12, 12]),
    'Lavender':  theme([200, 180, 255], [16, 12, 24], [9, 7, 15]),
    'Crimson':   theme([220, 20, 60], [20, 6, 10], [11, 3, 6]),
    'Gold':      theme([212, 175, 55], [20, 16, 8], [11, 9, 4]),
    'Mint':      theme([152, 255, 204], [8, 20, 16], [4, 12, 9]),
    'Rainbow':   rainbowTheme(0),                  // dynamic: replaced on every tick while selected
  };

  // Window background alpha presets (0-255)
  const OPACITY_PRESETS = [['Clear', 70], ['Glass', 110], ['Frosted', 160], ['Solid', 225]];
  const DEFAULT_ALPHA = 110;

  // ------------------------------------------------------------- state

  const S = {            // persisted settings + derived theme
    themeName: 'Pink',
    theme: THEMES.Pink,
    bgAlpha: DEFAULT_ALPHA,
    playlists: {},
    repeat: false,
    shuffle: false,
    discordOn: true,
    volume: 0.8,
  };

  const P = {            // playback / recovery state
    tracks: [],          // tracks shown in the list: {id, title, dur, secs}
    queue: [],           // tracks actually being played (kept when browsing other playlists)
    currentIndex: -1,
    currentPlaylist: '',
    queuePlaylist: '',   // playlist the queue came from (for Discord)
    shuffleBag: [],
    shuffleHistory: [],
    playlistReq: 0,
    streamReq: 0,
    lastError: '',

    resumeMs: 0,         // position to seek to once the new stream has loaded
    resumeFloor: 0,      // ignore positions below this until the resume seek lands
    resumeBase: 0,       // position where the current (re)start began
    lastPos: 0,          // last real playback position
    expectedMs: 0,       // real track length according to yt-dlp
    watchPos: -1,
    lastProgress: nowS(),
    loadStarted: nowS(),
    active: false,       // a track is supposed to be playing
    resolving: false,    // waiting for yt-dlp to give us a stream URL
    loading: false,      // source set, playback not yet confirmed
    endHandled: false,   // guards against double "track ended" handling
    failCount: 0,
    skipStreak: 0,
    authBlocked: false,
    allowJumpUntil: 0,   // until then a backwards position jump is a real seek
    guardTime: 0,
    guardCount: 0,
    resumeTries: 0,
    resumeTryTime: 0,
    userPaused: false,
    pausedSince: 0,      // when playback was found paused without the user asking for it
    blockedCount: 0,     // consecutive bot-check blocks (drives the retry back-off)
    deviceGraceUntil: 0,
    lastSec: -1,
    preload: null,       // { id, url, el, at }: the next track, resolved and already buffering in its own element
    prefetchJob: null,   // { id, promise }: lookup of the next track that is still running
    prefetchBusy: false,
    prefetchFailAt: 0,
    idleSince: 0,        // since when the player has been stopped with nothing scheduled
  };

  let cfg = { presence_enabled: false, discord_icon_key: 'harmony' };
  let hasCookies = false;
  let sidebarExpanded = true;
  let modalOpen = 0;     // open dialogs / menus (keyboard shortcuts are paused meanwhile)

  // `let`: the preloaded next track is a second element that replaces this one when it is due.
  let audio = new Audio();
  audio.preload = 'auto';

  // ------------------------------------------------------------- persistence

  function persist() {
    const settings = {
      theme_name: S.themeName,
      bg_alpha: S.bgAlpha,
      volume: audio.volume,
      repeat: S.repeat,
      shuffle: S.shuffle,
      playlists: S.playlists,
      discord_rpc: S.discordOn,
    };
    return invoke('save_settings', { settings }).catch(() => {});
  }

  let volumeSaveTimer = 0;
  const saveVolumeSoon = () => {
    clearTimeout(volumeSaveTimer);
    volumeSaveTimer = setTimeout(persist, 500);
  };

  // ------------------------------------------------------------- vector buttons

  const ICONS = {
    play: '<path d="M13 10L26 18L13 26Z"/>',
    pause: '<rect x="12" y="10" width="4" height="16" rx="1"/><rect x="20" y="10" width="4" height="16" rx="1"/>',
    next: '<path d="M11 11L19 18L11 25Z"/><rect x="20" y="11" width="3" height="14" rx="1"/>',
    prev: '<path d="M25 11L17 18L25 25Z"/><rect x="13" y="11" width="3" height="14" rx="1"/>',
    repeat: '<path d="M23.36 13.5A7 7 0 1 0 23.36 22.5" fill="none" stroke="currentColor" stroke-width="2"/>' +
            '<path d="M22 10L27 14L21 17Z"/>',
    shuffle: '<g fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">' +
             '<path d="M9 13C17 13 16 23 23 23"/><path d="M9 23C17 23 16 13 23 13"/></g>' +
             '<path d="M22 9.5L27.5 13L22 16.5Z"/><path d="M22 19.5L27.5 23L22 26.5Z"/>',
  };

  class VectorButton {
    constructor(el, type) {
      this.el = el;
      this.type = type;
      this.el.innerHTML =
        '<svg viewBox="0 0 36 36" width="36" height="36">' +
        `<defs><radialGradient id="glow-${el.id}">` +
        '<stop offset="0" stop-color="currentColor" stop-opacity="0.549"/>' +
        '<stop offset="0.55" stop-color="currentColor" stop-opacity="0.275"/>' +
        '<stop offset="1" stop-color="currentColor" stop-opacity="0"/></radialGradient></defs>' +
        `<g class="glow"><circle cx="18" cy="18" r="18" fill="url(#glow-${el.id})"/>` +
        '<circle cx="18" cy="18" r="14.5" fill="none" stroke="currentColor" stroke-opacity="0.667" stroke-width="1.2"/></g>' +
        '<g class="icon"></g></svg>';
      this.icon = el.querySelector('.icon');
      this.setIcon(type);
    }
    setIcon(type) { this.type = type; this.icon.innerHTML = ICONS[type]; }
    setActive(on) { this.el.classList.toggle('active', on); }
  }

  const btn = {
    shuffle: new VectorButton($('btn-shuffle'), 'shuffle'),
    prev: new VectorButton($('btn-prev'), 'prev'),
    play: new VectorButton($('btn-play'), 'play'),
    next: new VectorButton($('btn-next'), 'next'),
    repeat: new VectorButton($('btn-repeat'), 'repeat'),
  };

  // ------------------------------------------------------------- wave visualizer

  const viz = {
    canvas: $('viz'),
    W: 220,
    H: 140,
    // [frequency, amplitude multiplier, alpha, pen width, phase shift]
    WAVES: [[0.02, 1.0, 230, 2.2, 0.0], [0.035, 0.6, 150, 1.5, 1.2], [0.015, 0.4, 90, 1.0, 2.5]],
    STEP: 4,               // px between wave samples
    playing: false,
    active: true,
    phase: 0,
    amp: 0.05,
    color: [255, 182, 193],
    timer: 0,

    init() {
      const dpr = window.devicePixelRatio || 1;
      this.canvas.width = this.W * dpr;
      this.canvas.height = this.H * dpr;
      this.ctx = this.canvas.getContext('2d');
      this.ctx.scale(dpr, dpr);
      this.xs = [];
      this.env = [];
      for (let x = 0; x <= this.W; x += this.STEP) {
        this.xs.push(x);
        this.env.push(Math.sin((x / this.W) * Math.PI));
      }
      this.schedule();
    },
    interval() { return this.playing ? 33 : 83; },   // ~30 fps playing, ~12 fps idle
    schedule() {
      clearTimeout(this.timer);
      if (this.active) this.timer = setTimeout(() => { this.animate(); this.schedule(); }, this.interval());
    },
    setColor(rgb) { this.color = rgb; },
    setPlaying(on) { this.playing = on; },
    setActive(on) { this.active = on; if (on) this.schedule(); else clearTimeout(this.timer); },
    animate() {
      if (this.playing) { this.amp += (0.35 - this.amp) * 0.1; this.phase += 0.165; }
      else { this.amp += (0.05 - this.amp) * 0.18; this.phase += 0.1; }
      this.draw();
    },
    draw() {
      const { ctx, W, H } = this;
      ctx.clearRect(0, 0, W, H);
      const mid = H / 2, base = H * this.amp;
      const [r, g, b] = this.color;
      ctx.lineJoin = 'bevel';
      ctx.lineCap = 'square';
      for (const [freq, mult, alpha, width, shift] of this.WAVES) {
        const amp = base * mult, ph = this.phase + shift;
        ctx.strokeStyle = `rgba(${r},${g},${b},${alpha / 255})`;
        ctx.lineWidth = width;
        ctx.beginPath();
        this.xs.forEach((x, i) => {
          const y = mid + Math.sin(x * freq + ph) * amp * this.env[i];
          if (i) ctx.lineTo(x, y); else ctx.moveTo(x, y);
        });
        ctx.stroke();
      }
    },
  };

  // ------------------------------------------------------------- theme application

  function applyTheme() {
    const t = S.theme, root = document.documentElement.style;
    root.setProperty('--acc', hex(t.accent));
    root.setProperty('--acc-rgb', t.accent.join(','));
    root.setProperty('--bg-rgb', t.bg.join(','));
    root.setProperty('--sb-rgb', t.sidebar_bg.join(','));
    root.setProperty('--bg-a', (S.bgAlpha / 255).toFixed(3));
    root.setProperty('--sb-a', (Math.min(255, Math.floor(S.bgAlpha * 0.6) + 25) / 255).toFixed(3));
    viz.setColor(t.accent);
  }

  function changeTheme(name) {
    if (!THEMES[name]) return;
    S.themeName = name;
    S.theme = THEMES[name];
    applyTheme();
    syncRainbowTimer();
    persist();
  }

  function setTransparency(alpha) {
    S.bgAlpha = alpha;
    applyTheme();
    persist();
  }

  let rainbowTimer = 0;
  function syncRainbowTimer() {
    clearInterval(rainbowTimer);
    rainbowTimer = 0;
    if (S.themeName === 'Rainbow' && !document.hidden) {
      rainbowTimer = setInterval(() => {
        S.theme = rainbowTheme((nowS() / RAINBOW_CYCLE_SEC) % 1);
        THEMES.Rainbow = S.theme;
        applyTheme();
      }, 200);
    }
  }

  // ------------------------------------------------------------- track list (virtualised)

  const list = {
    el: $('list'),
    inner: $('list-inner'),
    tracks: [],
    message: '',
    selected: -1,
    hlRow: -1,
    hlColor: null,

    count() { return this.tracks.length || (this.message ? 1 : 0); },

    setTracks(tracks, message = '') {
      this.tracks = tracks;
      this.message = message;
      this.selected = -1;
      this.hlRow = -1;
      this.hlColor = null;
      this.el.scrollTop = 0;
      this.render();
    },

    rowEl(i) { return this.inner.querySelector(`.row[data-i="${i}"]`); },

    render() {
      const n = this.count();
      this.inner.style.height = `${n * ROW_H}px`;
      const first = Math.max(0, Math.floor(this.el.scrollTop / ROW_H) - 2);
      const last = Math.min(n - 1, Math.ceil((this.el.scrollTop + this.el.clientHeight) / ROW_H) + 2);
      const frag = document.createDocumentFragment();
      for (let i = first; i <= last; i++) {
        const row = document.createElement('div');
        row.className = 'row' + (i === this.selected ? ' sel' : '');
        row.dataset.i = i;
        row.style.top = `${i * ROW_H}px`;
        if (this.tracks.length) {
          const t = this.tracks[i];
          row.textContent = `${String(i + 1).padStart(2, '0')}. ${t.title} [${t.dur}]`;
        } else {
          row.textContent = this.message;
        }
        if (i === this.hlRow && this.hlColor) row.style.color = this.hlColor;
        frag.appendChild(row);
      }
      this.inner.replaceChildren(frag);
    },

    select(i) {
      this.rowEl(this.selected)?.classList.remove('sel');
      this.selected = i;
      this.rowEl(i)?.classList.add('sel');
    },

    setCurrent(i) {
      this.select(i);
      const top = i * ROW_H, bottom = top + ROW_H;
      if (top < this.el.scrollTop) this.el.scrollTop = top;
      else if (bottom > this.el.scrollTop + this.el.clientHeight) this.el.scrollTop = bottom - this.el.clientHeight;
      this.render();
    },

    setHighlight(row, color) {
      const old = this.rowEl(this.hlRow);
      if (old) old.style.color = '';
      this.hlRow = row;
      this.hlColor = color;
      const el = this.rowEl(row);
      if (el && color) el.style.color = color;
    },
  };

  let renderQueued = false;
  list.el.addEventListener('scroll', () => {
    if (renderQueued) return;
    renderQueued = true;
    requestAnimationFrame(() => { renderQueued = false; list.render(); });
  });
  list.inner.addEventListener('click', (e) => {
    const row = e.target.closest('.row');
    if (row) list.select(Number(row.dataset.i));
  });
  list.inner.addEventListener('dblclick', (e) => {
    const row = e.target.closest('.row');
    if (row) onTrackSelected(Number(row.dataset.i));
  });
  new ResizeObserver(() => list.render()).observe(list.el);

  // ------------------------------------------------------------- now-playing highlight

  let glowTimer = 0;
  function startGlow() {
    if (!glowTimer) glowTimer = setInterval(updateBreathingLight, 80);   // ~12 Hz is plenty for a 2 s pulse
  }
  function stopGlow() {
    clearInterval(glowTimer);
    glowTimer = 0;
  }
  function updateBreathingLight() {
    if (P.queue !== P.tracks || P.currentIndex < 0) return;
    // alpha 110 -> 255 -> 110, 2 second period
    const a = Math.round(182 - 72 * Math.cos(nowS() * Math.PI));
    const [r, g, b] = S.theme.accent;
    list.setHighlight(P.currentIndex, `rgba(${r},${g},${b},${a / 255})`);
  }
  function clearGlow() {
    if (list.hlRow >= 0) list.setHighlight(-1, null);
  }

  // ------------------------------------------------------------- progress slider

  const slider = {
    el: $('slider'),
    fill: document.querySelector('#slider .fill'),
    handle: document.querySelector('#slider .handle'),
    max: 0,
    value: 0,
    down: false,

    setRange(max) { this.max = Math.max(0, max); this.setValue(this.value); },
    setValue(v) {
      this.value = clamp(v, 0, this.max);
      const f = this.max > 0 ? this.value / this.max : 0;
      this.handle.style.left = `calc((100% - 10px) * ${f})`;
      this.fill.style.width = `calc((100% - 10px) * ${f} + 5px)`;
    },
    valueAt(e) {
      const rect = this.el.getBoundingClientRect();
      return Math.round(clamp((e.clientX - rect.left) / Math.max(1, rect.width), 0, 1) * this.max);
    },
  };

  slider.el.addEventListener('pointerdown', (e) => {
    if (e.button !== 0 || slider.max <= 0) return;
    slider.el.setPointerCapture(e.pointerId);
    slider.down = true;
    const v = slider.valueAt(e);
    slider.setValue(v);
    ui.cur.textContent = fmt(v / 1000);
  });
  slider.el.addEventListener('pointermove', (e) => {
    if (!slider.down) return;
    const v = slider.valueAt(e);
    slider.setValue(v);
    ui.cur.textContent = fmt(v / 1000);
  });
  slider.el.addEventListener('pointerup', () => {
    if (!slider.down) return;
    slider.down = false;
    onSliderReleased();      // seek only on release (avoids spamming a network stream)
  });

  // ------------------------------------------------------------- ui references

  const ui = {
    label: $('pl-label'),
    now: $('now'),
    cur: $('time-cur'),
    total: $('time-total'),
    volume: $('volume'),
    tree: $('tree'),
    sidebar: $('sidebar'),
  };

  // ------------------------------------------------------------- audio helpers

  const setPosition = (ms) => { audio.currentTime = ms / 1000; };
  const positionMs = () => Math.floor(audio.currentTime * 1000);
  const mediaDurationMs = () => (Number.isFinite(audio.duration) ? Math.floor(audio.duration * 1000) : 0);
  const effectiveDurationMs = () => mediaDurationMs() || P.expectedMs;

  // 'playing' | 'paused' | 'stopped'
  function playbackState() {
    if (!audio.getAttribute('src') || audio.error || audio.ended) return 'stopped';
    return audio.paused ? 'paused' : 'playing';
  }

  // ------------------------------------------------------------- playlists

  function updateSidebarTree() {
    ui.tree.replaceChildren(...Object.keys(S.playlists).map((name) => {
      const node = document.createElement('div');
      node.className = 'tnode';
      node.dataset.name = name;
      node.textContent = `📂 ${name}`;
      return node;
    }));
  }

  function selectTreeNode(node) {
    ui.tree.querySelector('.tnode.sel')?.classList.remove('sel');
    node?.classList.add('sel');
  }

  ui.tree.addEventListener('click', (e) => selectTreeNode(e.target.closest('.tnode')));
  ui.tree.addEventListener('dblclick', (e) => {
    const node = e.target.closest('.tnode');
    if (node) switchPlaylist(node.dataset.name);
  });
  ui.tree.addEventListener('contextmenu', (e) => {
    const node = e.target.closest('.tnode');
    if (!node) return;
    const name = node.dataset.name;
    showMenu([{ label: `🗑 Remove '${name}'`, onClick: () => removePlaylist(name) }], e.clientX, e.clientY);
  });

  async function addPlaylist() {
    const name = await ask('Playlist Name', 'Enter a name for this playlist:');
    if (!name) return;
    const url = await ask('YouTube Link', 'Paste YouTube Playlist URL:');
    if (!url || !/^https?:\/\//i.test(url)) return;
    S.playlists[name] = url;
    persist();
    updateSidebarTree();
    switchPlaylist(name);
  }

  function removePlaylist(name) {
    if (!(name in S.playlists)) return;
    delete S.playlists[name];
    persist();
    updateSidebarTree();
    if (name === P.currentPlaylist) {
      P.playlistReq++;
      P.currentPlaylist = '';
      P.tracks = [];
      list.setTracks(P.tracks);
      ui.label.textContent = 'No Playlist Selected';
    }
  }

  async function switchPlaylist(name) {
    if (!(name in S.playlists)) return;
    P.currentPlaylist = name;
    ui.label.textContent = name;
    P.tracks = [];
    list.setTracks(P.tracks, 'Loading tracks...');
    const req = ++P.playlistReq;
    let result;
    try {
      result = await invoke('fetch_playlist', { req, url: S.playlists[name] });
    } catch (e) {
      result = { tracks: [], error: String(e) };
    }
    populateTracks(req, result);
  }

  function populateTracks(req, { tracks, error }) {
    if (req !== P.playlistReq) return;       // stale result from a previous click
    P.lastError = error || '';
    P.tracks = tracks;
    list.setTracks(tracks, tracks.length ? '' :
      'No tracks found or error loading playlist.' + (P.lastError ? ` (${P.lastError.slice(0, 90)})` : ''));
  }

  function onTrackSelected(row) {
    if (P.tracks.length && row >= 0 && row < P.tracks.length) {
      P.queue = P.tracks;
      P.queuePlaylist = P.currentPlaylist;
      P.skipStreak = 0;
      resetShuffle();
      playTrack(row);
    }
  }

  // ------------------------------------------------------------- playback

  async function playTrack(index, resumeMs = 0, retry = false) {
    if (!(index >= 0 && index < P.queue.length)) return;
    if (!retry) P.failCount = 0;
    stopRetry();
    clearGlow();
    P.currentIndex = index;
    const track = P.queue[index];
    P.expectedMs = (track.secs || 0) * 1000;
    P.resumeMs = resumeMs;
    P.resumeFloor = resumeMs;          // ignore bogus 0:00 positions until the seek lands
    P.resumeBase = resumeMs;
    P.lastPos = resumeMs;
    P.watchPos = -1;
    P.lastProgress = P.loadStarted = nowS();
    P.endHandled = false;
    P.userPaused = false;
    P.guardCount = 0;
    P.resumeTries = 0;
    P.resumeTryTime = 0;
    P.allowJumpUntil = nowS() + 3;
    P.active = true;
    P.resolving = true;
    P.loading = true;
    if (P.queue === P.tracks) list.setCurrent(index);
    ui.now.textContent = (retry ? 'Reconnecting: ' : 'Loading: ') + track.title;

    // Silence the previous song right away. Left alone it keeps playing (and firing stale events)
    // while the new stream is looked up, which looks like being stuck on the old track.
    audio.pause();
    audio.removeAttribute('src');
    audio.load();
    P.lastSec = -1;
    slider.setValue(resumeMs);
    ui.cur.textContent = fmt(resumeMs / 1000);
    updateDuration();

    const req = ++P.streamReq;
    const reusable = !retry && resumeMs === 0;
    let pre = reusable ? takePreload(track.id) : null;
    if (!pre && reusable && P.prefetchJob && P.prefetchJob.id === track.id) {
      // the background lookup for exactly this track is already running: join it, don't start over
      await Promise.race([P.prefetchJob.promise, new Promise((r) => setTimeout(r, 20000))]);
      if (req !== P.streamReq) return;
      pre = takePreload(track.id);
    }
    if (pre) {                          // resolved and already buffering: start instantly
      startStream(req, { url: pre.url, el: pre.el, offline: false, auth_blocked: false });
      return;
    }
    let result;
    try {
      result = await invoke('fetch_stream', { req, videoId: track.id, retry });
    } catch (e) {
      result = { url: '', offline: false, auth_blocked: false, error: String(e) };
    }
    if (result.error) P.lastError = result.error;
    startStream(req, result);
  }

  function startStream(req, { url, el, offline, auth_blocked: authBlocked }) {
    if (req !== P.streamReq) return;
    P.resolving = false;
    P.authBlocked = !!authBlocked;
    if (!url) {
      onResolveFailed(offline);
      return;
    }
    if (P.currentIndex >= 0 && P.currentIndex < P.queue.length) ui.now.textContent = P.queue[P.currentIndex].title;
    const t = nowS();
    P.loadStarted = P.lastProgress = t;
    P.lastPos = P.resumeMs;
    P.watchPos = -1;
    if (el) adoptPreloaded(el);
    else audio.src = url;
    updateDuration();
    audio.play().catch((err) => {
      const reason = `${err?.name || 'PlaybackError'}: ${err?.message || String(err)}`;
      P.lastError = `audio.play() rejected - ${reason}`;
      console.warn('[Harmony] audio.play() rejected', {
        trackId: P.queue[P.currentIndex]?.id,
        reason,
        mediaErrorCode: audio.error?.code ?? null,
        readyState: audio.readyState,
        networkState: audio.networkState,
      });
    });
    refreshPresence();
  }

  // ------------------------------------------------------------- prefetch next track

  const PREFETCH_TTL_S = 2 * 3600;      // stream URLs stay valid for hours; stay well inside that
  const PREFETCH_RETRY_S = 15;

  function disposeAudio(el) {
    el.removeAttribute('src');
    el.load();                           // drops the connection and the buffered data
  }

  function discardPreload() {
    if (P.preload) disposeAudio(P.preload.el);
    P.preload = null;
  }

  // The next track's element, if it is ready for `id`. Taking it hands ownership to the caller.
  function takePreload(id) {
    const c = P.preload;
    if (!c || c.id !== id) return null;
    P.preload = null;
    if (c.el.error || nowS() - c.at > PREFETCH_TTL_S) {
      disposeAudio(c.el);
      return null;
    }
    return c;
  }

  function peekNextIndex() {
    const n = P.queue.length;
    if (n <= 1 || P.currentIndex < 0) return -1;
    if (S.shuffle) {
      ensureShuffleBag();
      return P.shuffleBag[P.shuffleBag.length - 1];
    }
    return (P.currentIndex + 1) % n;
  }

  // While a song plays, look up the next one AND start buffering it in a second audio element, so
  // the switch needs neither a network lookup nor a wait for the first bytes.
  function maybePrefetch() {
    if (P.prefetchBusy || !P.active || P.resolving || P.loading || S.repeat || retryPending()) return;
    if (playbackState() !== 'playing' || P.lastPos < 3000) return;
    if (nowS() - P.prefetchFailAt < PREFETCH_RETRY_S) return;
    const idx = peekNextIndex();
    if (idx < 0 || idx === P.currentIndex) return;
    const track = P.queue[idx];
    const c = P.preload;
    if (c && c.id === track.id && !c.el.error && nowS() - c.at < PREFETCH_TTL_S) return;
    discardPreload();                    // stale: other track / failed / too old
    P.prefetchBusy = true;
    const job = (async () => {
      try {
        // Same request id as the track that is playing now: the backend lets any newer foreground
        // lookup cancel this one, and this one never cancels a foreground lookup.
        const res = await invoke('fetch_stream', { req: P.streamReq, videoId: track.id, retry: false });
        if (res && res.url) {
          if (P.active && peekNextIndex() >= 0 && P.queue[peekNextIndex()]?.id === track.id) {
            const el = new Audio();
            el.preload = 'auto';
            el.src = res.url;
            el.load();
            discardPreload();
            P.preload = { id: track.id, url: res.url, el, at: nowS() };
          }
        } else if (!res || res.error !== 'superseded') {
          P.prefetchFailAt = nowS();     // abandoned for a newer track: not a failure
        }
      } catch (e) {
        P.prefetchFailAt = nowS();
      }
      P.prefetchBusy = false;
      if (P.prefetchJob && P.prefetchJob.id === track.id) P.prefetchJob = null;
    })();
    P.prefetchJob = { id: track.id, promise: job };
  }

  // ------------------------------------------------------------- recovery

  function isNearEnd(pos) {
    const lengths = [mediaDurationMs(), P.expectedMs].filter((x) => x > 0);
    return lengths.length > 0 && pos >= Math.min(...lengths) - END_TOLERANCE_MS;
  }

  let retryTimer = 0;
  const retryPending = () => retryTimer !== 0;
  function stopRetry() { clearTimeout(retryTimer); retryTimer = 0; }
  function scheduleRetry(ms) {
    if (retryPending()) return;
    retryTimer = setTimeout(() => { retryTimer = 0; doRetry(); }, ms);
  }
  function doRetry() {
    if (P.active && P.currentIndex >= 0 && P.currentIndex < P.queue.length) {
      playTrack(P.currentIndex, P.lastPos, true);
    }
  }

  function onResolveFailed(offline) {
    if (P.currentIndex < 0) return;
    if (offline) {                       // no internet: keep polling, never give up
      ui.now.textContent = 'Waiting for connection...';
      scheduleRetry(OFFLINE_POLL_MS);
      return;
    }
    if (P.authBlocked) {
      // skipping cannot help (every track gets blocked), so keep retrying this one with a growing delay
      P.blockedCount++;
      ui.now.textContent = hasCookies
        ? 'YouTube still blocks this track. Cookies may be stale or a playback token may be required - retrying...'
        : 'YouTube blocked this track - right-click the icon > Import cookies.txt, then retry.';
      // first retry is immediate, then 3s, 6s, 12s ... up to 30s
      scheduleRetry(P.blockedCount === 1 ? 0 : Math.min(SLOW_RETRY_MS, 3000 * 2 ** (P.blockedCount - 2)));
      return;
    }
    P.failCount++;
    if (P.failCount >= MAX_RETRIES) giveUpTrack();
    else scheduleRetry(RETRY_DELAYS[Math.min(P.failCount - 1, RETRY_DELAYS.length - 1)]);
  }

  // Stream died, stalled or errored. Finish the song if it was basically over,
  // otherwise reconnect and resume from the last real position.
  function recover() {
    if (!P.active || P.resolving || P.endHandled || retryPending()) return;
    if (isNearEnd(P.lastPos)) {
      handleTrackEnd();
      return;
    }
    if (nowS() < P.deviceGraceUntil) {
      // audio device just changed: not a network problem, so don't count it as a failure
      if (!deviceTimer) deviceTimer = setTimeout(rebindAudio, 500);
      return;
    }
    P.failCount++;
    if (P.failCount >= MAX_RETRIES) {
      giveUpTrack();
      return;
    }
    ui.now.textContent = 'Reconnecting...';
    scheduleRetry(RETRY_DELAYS[Math.min(P.failCount - 1, RETRY_DELAYS.length - 1)]);
  }

  function giveUpTrack() {
    P.skipStreak++;
    const n = P.queue.length;
    if (n > 1 && P.skipStreak < Math.min(n, 5)) {
      ui.now.textContent = 'Track unavailable, skipping...';
      playNext();
    } else {
      // nothing else to skip to: keep trying slowly instead of stopping for good
      P.failCount = 0;
      P.skipStreak = 0;
      ui.now.textContent = 'Error: ' + (P.lastError.slice(0, 80) || 'stream unavailable') + ' - retrying...';
      scheduleRetry(SLOW_RETRY_MS);
    }
  }

  function handleTrackEnd() {
    if (P.endHandled || !P.active) return;
    P.endHandled = true;
    if (S.repeat && P.currentIndex >= 0) {
      P.endHandled = false;
      P.allowJumpUntil = nowS() + 3;
      P.lastPos = 0;
      P.resumeFloor = 0;
      P.resumeBase = 0;
      P.lastProgress = nowS();
      setPosition(0);
      audio.play().catch(() => {});
      refreshPresence();
    } else {
      playNext();
    }
  }

  // ------------------------------------------------------------- audio device changes

  let deviceTimer = 0;

  function onAudioDevicesChanged() {
    P.deviceGraceUntil = nowS() + 8;
    clearTimeout(deviceTimer);
    deviceTimer = setTimeout(rebindAudio, 700);   // debounce: Windows fires several events per change
  }

  function rebindAudio() {
    deviceTimer = 0;
    if (!P.active || P.userPaused || P.resolving || retryPending()) return;
    const state = playbackState();
    if (state === 'playing') return;
    if (state === 'paused') {                      // the system paused us when the device vanished
      audio.play().catch(() => {});
    } else if (P.currentIndex >= 0 && P.currentIndex < P.queue.length) {
      // stopped: reopen the stream and continue from where we were (not from 0:00)
      playTrack(P.currentIndex, P.lastPos, true);
    }
  }

  navigator.mediaDevices?.addEventListener('devicechange', onAudioDevicesChanged);

  function checkStopped() {
    if (!P.active || P.resolving || P.loading || P.endHandled || retryPending()) return;
    if (playbackState() !== 'stopped') return;
    if (isNearEnd(P.lastPos)) handleTrackEnd();
    else recover();
  }

  // 1 Hz safety net: catches missing end-of-track events, frozen streams and hung loads.
  function watchdogTick() {
    const now = nowS();
    maybePrefetch();
    // Stopped, nothing loading, no retry scheduled, not paused by the user = stuck. Move on.
    if (P.active && !P.resolving && !P.userPaused && !retryPending() && playbackState() === 'stopped') {
      P.idleSince = P.idleSince || now;
      if (now - P.idleSince > 4) {
        P.idleSince = 0;
        P.endHandled = false;
        if (audio.ended || isNearEnd(P.lastPos)) handleTrackEnd();
        else recover();
        return;
      }
    } else {
      P.idleSince = 0;
    }
    if (!P.active || P.resolving || P.endHandled || retryPending()) return;
    const state = playbackState();
    if (state === 'paused') {
      if (P.userPaused) {              // paused by the user: nothing to watch
        P.pausedSince = 0;
      } else {                         // never asked for: the player refused to start, or the system paused it
        P.pausedSince = P.pausedSince || now;
        audio.play().catch(() => {});
        if (now - P.pausedSince > 15) {
          P.pausedSince = 0;
          recover();
        }
      }
      P.lastProgress = now;
      P.loadStarted = now;
      return;
    }
    P.pausedSince = 0;
    if (P.resumeMs > 0 && state === 'playing' && audio.readyState >= 3 && now - P.resumeTryTime > 2.0) {
      if (P.resumeTries < 6) {         // the seek didn't stick (song would restart): try again
        P.resumeTries++;
        P.resumeTryTime = now;
        setPosition(P.resumeMs);
      } else {
        P.resumeMs = 0;
        P.resumeFloor = 0;
      }
    }
    if (P.loading) {
      if (now - P.loadStarted > LOAD_TIMEOUT_S) recover();
      return;
    }
    if (state !== 'playing') {
      P.lastProgress = now;
      return;
    }
    const stalled = now - P.lastProgress;
    if (stalled > 2.5 && isNearEnd(P.lastPos)) handleTrackEnd();   // 'ended' never arrived, but the song is over
    else if (stalled > STALL_TIMEOUT_S) recover();
  }
  setInterval(watchdogTick, 1000);

  // ------------------------------------------------------------- media events

  function onMediaLoaded() {
    if (P.resolving) return;                       // events from the previous source
    if (P.resumeMs > 0) {                          // cleared in updatePosition once the seek has landed
      setPosition(P.resumeMs);
      P.resumeTryTime = nowS();
    }
  }

  function onMediaEnded() {
    if (P.resolving) return;
    const durMs = mediaDurationMs();
    const posMs = positionMs();
    if (durMs > 0 && posMs >= durMs - 2000) {      // the element itself says it played to the end
      P.lastPos = Math.max(P.lastPos, posMs);
      handleTrackEnd();
      return;
    }
    const hasLength = mediaDurationMs() > 0 || P.expectedMs > 0;
    if (isNearEnd(P.lastPos) || !hasLength) handleTrackEnd();
    else recover();                                // stream was cut short
  }

  function onPlayerError() {
    if (P.resolving || !audio.getAttribute('src')) return;
    const mediaError = audio.error;
    const labels = {
      1: 'playback aborted',
      2: 'network error',
      3: 'audio decode error',
      4: 'unsupported media source',
    };
    const code = mediaError?.code ?? 0;
    const detail = `WebView media error ${code}${labels[code] ? ` (${labels[code]})` : ''}`;
    P.lastError = mediaError?.message ? `${detail}: ${mediaError.message}` : detail;
    console.warn('[Harmony] media element failed', {
      trackId: P.queue[P.currentIndex]?.id,
      detail: P.lastError,
      readyState: audio.readyState,
      networkState: audio.networkState,
    });
    recover();
  }

  let lastPlaybackState = 'stopped';
  function syncPlaybackState() {
    const state = playbackState();
    if (state === lastPlaybackState) return;
    lastPlaybackState = state;
    onPlaybackState(state);
  }

  function onPlaybackState(state) {
    const playing = state === 'playing';
    btn.play.setIcon(playing ? 'pause' : 'play');
    btn.play.setActive(playing);
    viz.setPlaying(playing);
    if (playing) {
      P.lastProgress = nowS();
      startGlow();
    } else {
      stopGlow();
      clearGlow();
      if (state === 'stopped' && P.active) setTimeout(checkStopped, 700);
    }
    refreshPresence();
  }

  // All player events are attached to whichever element is current; swapping in the preloaded
  // element detaches the old one first so it can never fire stale events.
  let audioListeners = null;
  function wireAudio(el) {
    audioListeners?.abort();
    audioListeners = new AbortController();
    const opt = { signal: audioListeners.signal };
    el.addEventListener('loadedmetadata', onMediaLoaded, opt);
    el.addEventListener('canplay', onMediaLoaded, opt);
    el.addEventListener('ended', onMediaEnded, opt);
    el.addEventListener('error', onPlayerError, opt);
    el.addEventListener('durationchange', updateDuration, opt);
    el.addEventListener('timeupdate', () => updatePosition(positionMs()), opt);
    el.addEventListener('seeked', () => updatePosition(positionMs()), opt);
    ['play', 'playing', 'pause', 'ended', 'error', 'emptied', 'loadstart'].forEach((ev) =>
      el.addEventListener(ev, syncPlaybackState, opt));
  }
  wireAudio(audio);

  function adoptPreloaded(el) {
    const old = audio;
    audioListeners?.abort();            // old element: no more events
    audioListeners = null;
    old.pause();
    old.removeAttribute('src');
    old.load();
    el.volume = old.volume;
    audio = el;
    wireAudio(audio);
  }
  setInterval(() => { if (playbackState() === 'playing') updatePosition(positionMs()); }, 100);

  function updatePosition(position) {
    if (P.resolving) return;                       // still the previous track
    if (position !== P.watchPos) {
      P.watchPos = position;
      P.lastProgress = nowS();
    }
    if (position > 0) P.loading = false;

    // after a reconnect, ignore the temporary 0:00 until the resume seek lands
    const now = nowS();
    let glitch = false;
    if (P.resumeMs === 0 && P.lastPos > 8000 && position + 5000 < P.lastPos
        && now > P.allowJumpUntil && !slider.down) {
      // position fell back by itself (stream reopened and started over): jump back, don't restart
      glitch = P.guardCount < 5;
      if (glitch && now - P.guardTime > 2.0 && playbackState() === 'playing') {
        P.guardTime = now;
        P.guardCount++;
        setPosition(P.lastPos);
      }
    }
    if (P.resumeMs > 0 && position + 3000 >= P.resumeMs) P.resumeMs = 0;   // resume seek has landed

    // a sudden 0:00 is the player resetting itself (device lost, stop), not real progress
    if (!glitch && position + 3000 >= P.resumeFloor && !(position === 0 && P.lastPos > 3000)) {
      P.lastPos = position;
      P.resumeFloor = 0;
    }

    if (!slider.down) {
      slider.setValue(position);
      const sec = Math.floor(position / 1000);
      if (sec !== P.lastSec) {
        P.lastSec = sec;
        ui.cur.textContent = fmt(sec);
      }
    }

    // played steadily for 8 s past the last (re)start -> forgive earlier failures
    if ((P.failCount || P.skipStreak) && P.resumeFloor === 0 && P.lastPos - P.resumeBase > 8000) {
      P.failCount = 0;
      P.skipStreak = 0;
      P.blockedCount = 0;
    }
  }

  function updateDuration() {
    const duration = effectiveDurationMs();
    slider.setRange(duration);
    ui.total.textContent = fmt(duration / 1000);
    if (duration > 0 && !P.expectedMs) refreshPresence();
  }

  // ------------------------------------------------------------- transport

  function togglePlay() {
    const state = playbackState();
    if (state === 'playing') {
      P.userPaused = true;
      audio.pause();
    } else if (state === 'paused') {
      P.userPaused = false;
      audio.play().catch(() => {});
    } else if (P.queue.length && P.currentIndex >= 0) {
      P.skipStreak = 0;
      if (P.active && (audio.ended || isNearEnd(P.lastPos)) && P.queue.length > 1) {
        playNext();                               // song had finished: continue with the next one
      } else {
        const resume = P.active && !isNearEnd(P.lastPos) ? P.lastPos : 0;
        playTrack(P.currentIndex, resume);
      }
    } else if (P.tracks.length) {
      P.queue = P.tracks;
      P.queuePlaylist = P.currentPlaylist;
      P.skipStreak = 0;
      resetShuffle();
      playTrack(0);
    }
  }

  if ('mediaSession' in navigator) {
    const ms = navigator.mediaSession;
    ms.setActionHandler('play', () => { P.userPaused = false; audio.play().catch(() => {}); });
    ms.setActionHandler('pause', () => { P.userPaused = true; audio.pause(); });
    ms.setActionHandler('nexttrack', playNext);
    ms.setActionHandler('previoustrack', playPrevious);
  }

  function resetShuffle() {
    P.shuffleBag = [];
    P.shuffleHistory = [];
  }

  // Every track plays once per round, in random order, before the bag is refilled.
  function ensureShuffleBag() {
    const n = P.queue.length;
    if (!P.shuffleBag.length) {
      P.shuffleBag = [...Array(n).keys()].filter((i) => i !== P.currentIndex);
      for (let i = P.shuffleBag.length - 1; i > 0; i--) {
        const j = Math.floor(Math.random() * (i + 1));
        [P.shuffleBag[i], P.shuffleBag[j]] = [P.shuffleBag[j], P.shuffleBag[i]];
      }
    }
  }

  function nextShuffleIndex() {
    if (P.queue.length <= 1) return 0;
    ensureShuffleBag();
    return P.shuffleBag.pop();
  }

  function playNext() {
    if (!P.queue.length) return;
    if (S.shuffle) {
      if (P.currentIndex >= 0) {
        P.shuffleHistory.push(P.currentIndex);
        P.shuffleHistory.splice(0, Math.max(0, P.shuffleHistory.length - 200));   // cap history length
      }
      playTrack(nextShuffleIndex());
    } else {
      playTrack((P.currentIndex + 1) % P.queue.length);
    }
  }

  function playPrevious() {
    if (!P.queue.length) return;
    if (S.shuffle && P.shuffleHistory.length) {
      playTrack(P.shuffleHistory.pop());
    } else {
      const n = P.queue.length;
      playTrack(((P.currentIndex - 1) % n + n) % n);
    }
  }

  function toggleRepeat() {
    S.repeat = !S.repeat;
    btn.repeat.setActive(S.repeat);
    persist();
  }

  function toggleShuffle() {
    S.shuffle = !S.shuffle;
    resetShuffle();
    btn.shuffle.setActive(S.shuffle);
    persist();
  }

  // ------------------------------------------------------------- volume / seeking

  function refreshVolumeLabel() {
    const v = audio.volume;
    ui.volume.textContent = v <= 0.01 ? 'VOL MUTE' : `VOL ${Math.round(v * 100)}%`;
  }

  function setVolume(v) {
    audio.volume = clamp(v, 0, 1);
    refreshVolumeLabel();
    saveVolumeSoon();                              // debounce disk writes
  }

  const adjustVolume = (delta) => setVolume(audio.volume + delta);

  function seekBy(ms) {
    const dur = effectiveDurationMs();
    if (dur <= 0) return;
    const target = clamp(positionMs() + ms, 0, dur);
    P.allowJumpUntil = nowS() + 3;
    P.lastPos = target;
    P.resumeFloor = 0;
    setPosition(target);
    refreshPresence();
  }

  function onSliderReleased() {
    const target = slider.value;
    P.allowJumpUntil = nowS() + 3;
    P.lastPos = target;
    P.resumeFloor = 0;
    setPosition(target);
    refreshPresence();
  }

  ui.volume.addEventListener('wheel', (e) => {
    e.preventDefault();
    adjustVolume(e.deltaY < 0 ? 0.05 : -0.05);
  }, { passive: false });

  // ------------------------------------------------------------- discord status

  function refreshPresence() {
    if (!cfg.presence_enabled) return;
    const send = (activity) => invoke('set_presence', { activity }).catch(() => {});
    if (!S.discordOn || !P.active || !(P.currentIndex >= 0 && P.currentIndex < P.queue.length)) {
      send(null);
      return;
    }
    const state = playbackState();
    if (state === 'stopped') return;               // between tracks / reconnecting: keep what is shown

    const track = P.queue[P.currentIndex];
    let title = (track.title || 'Unknown Track').slice(0, 120);
    if (title.length < 2) title += ' \u266a';
    const playlist = P.queuePlaylist;
    const act = {
      details: title,
      state: playlist.length >= 2 ? playlist.slice(0, 100) : 'Harmony',
      large_image: `https://i.ytimg.com/vi/${track.id}/mqdefault.jpg`,   // video thumbnail
      large_text: title,
      small_image: cfg.discord_icon_key,                                   // Harmony icon
      small_text: 'Harmony',
      buttons: [{ label: 'Listen on YouTube', url: `https://www.youtube.com/watch?v=${track.id}` }],
    };
    if (state === 'playing') {
      const length = P.expectedMs || mediaDurationMs();
      const start = Math.floor(Date.now() / 1000) - Math.floor(P.lastPos / 1000);
      act.start = start;
      if (length > 0) act.end = start + Math.floor(length / 1000);
    } else {
      act.state = playlist.length >= 2 ? `Paused \u00b7 ${playlist}`.slice(0, 120) : 'Paused';
    }
    send(act);
  }

  function toggleDiscord() {
    S.discordOn = !S.discordOn;
    persist();
    refreshPresence();
  }

  // ------------------------------------------------------------- popup menu

  let closeMenu = null;

  function buildMenu(items, isSub) {
    const menu = document.createElement('div');
    menu.className = 'menu';
    for (const item of items) {
      const row = document.createElement('div');
      row.className = 'mi';
      if (item.checked !== undefined) {
        const chk = document.createElement('span');
        chk.className = 'chk';
        chk.textContent = item.checked ? '✓' : '';
        row.appendChild(chk);
      }
      const label = document.createElement('span');
      label.textContent = item.label;
      row.appendChild(label);
      if (item.sub) {
        const arrow = document.createElement('span');
        arrow.className = 'arrow';
        arrow.textContent = '▸';
        row.appendChild(arrow);
        const sub = buildMenu(item.sub, true);
        row.appendChild(sub);
        row.addEventListener('mouseenter', () => placeSubmenu(sub));
      } else {
        row.addEventListener('click', (e) => {
          e.stopPropagation();
          closeMenu?.();
          item.onClick?.();
        });
      }
      menu.appendChild(row);
    }
    return menu;
  }

  // Keeps a submenu inside the window (flip left / shift up, scroll if it is still too tall).
  function placeSubmenu(sub) {
    sub.style.top = '-5px';
    sub.style.left = '100%';
    sub.style.right = 'auto';
    sub.style.maxHeight = `${window.innerHeight - 12}px`;
    requestAnimationFrame(() => {
      const r = sub.getBoundingClientRect();
      if (r.right > window.innerWidth - 4) {
        sub.style.left = 'auto';
        sub.style.right = '100%';
      }
      const shift = r.bottom - (window.innerHeight - 6);
      if (shift > 0) sub.style.top = `${-5 - shift}px`;
      const r2 = sub.getBoundingClientRect();
      if (r2.top < 6) sub.style.top = `${parseFloat(sub.style.top) + (6 - r2.top)}px`;
    });
  }

  function showMenu(items, x, y) {
    closeMenu?.();
    const overlay = document.createElement('div');
    overlay.id = 'overlay';
    const menu = buildMenu(items, false);
    overlay.appendChild(menu);
    document.body.appendChild(overlay);
    modalOpen++;

    menu.style.left = `${x}px`;
    menu.style.top = `${y}px`;
    const r = menu.getBoundingClientRect();
    menu.style.left = `${Math.max(4, Math.min(x, window.innerWidth - r.width - 4))}px`;
    menu.style.top = `${Math.max(4, Math.min(y, window.innerHeight - r.height - 4))}px`;

    const onKey = (e) => { if (e.key === 'Escape') closeMenu?.(); };
    closeMenu = () => {
      closeMenu = null;
      document.removeEventListener('keydown', onKey, true);
      overlay.remove();
      modalOpen--;
    };
    overlay.addEventListener('mousedown', (e) => { if (e.target === overlay) closeMenu?.(); });
    overlay.addEventListener('contextmenu', (e) => { e.preventDefault(); if (e.target === overlay) closeMenu?.(); });
    document.addEventListener('keydown', onKey, true);
  }

  // ------------------------------------------------------------- cookies

  function importCookies() {
    const input = document.createElement('input');
    input.type = 'file';
    input.accept = '.txt,text/plain';
    input.addEventListener('change', async () => {
      const file = input.files[0];
      if (!file) return;
      try {
        await invoke('save_cookies', { text: await file.text() });
        hasCookies = true;
        ui.now.textContent = 'Cookies imported. Double-click a track to play.';
        P.active = false;
      } catch (e) {
        ui.now.textContent = String(e);
      }
    });
    input.click();
  }

  async function removeCookies() {
    await invoke('clear_cookies').catch(() => {});
    hasCookies = false;
    ui.now.textContent = 'Cookies removed.';
  }

  function showCookieGuide() {
    return info('How to get your cookies.txt',
      '1. Open a private/incognito window and sign in to YouTube (a spare account is safest).\n' +
      '2. Open a second tab at https://www.youtube.com/robots.txt, then close the original YouTube tab without reopening it.\n' +
      '3. While the private window is still open, export youtube.com cookies in Netscape/cookies.txt format with a reputable export extension (allow it in private mode if needed).\n' +
      '4. Right-click the Harmony icon > Import cookies.txt, then close the private window without reopening YouTube.\n\n' +
      'Cookies are login-session credentials. Keep the file private; Harmony stores it locally on this PC.');
  }

  // Theme / transparency menu. Only opened by right-clicking the title-bar icon.
  function showMainMenu(x, y) {
    const items = [
      {
        label: '🎨 Change Theme',
        sub: Object.keys(THEMES).map((t) => ({
          label: t, checked: t === S.themeName, onClick: () => changeTheme(t),
        })),
      },
      {
        label: '🪟 Transparency',
        sub: OPACITY_PRESETS.map(([label, alpha]) => ({
          label, checked: alpha === S.bgAlpha, onClick: () => setTransparency(alpha),
        })),
      },
    ];
    items.push({ label: '🍪 Import cookies.txt', onClick: importCookies });
    items.push({ label: '❓ How to get cookies', onClick: showCookieGuide });
    if (hasCookies) items.push({ label: '🧹 Remove cookies', onClick: removeCookies });
    if (cfg.presence_enabled) {
      items.push({ label: '💬 Discord Status', checked: S.discordOn, onClick: toggleDiscord });
    }
    showMenu(items, x, y);
  }

  // ------------------------------------------------------------- dialog

  function ask(title, label) {
    return new Promise((resolve) => {
      const wrap = document.createElement('div');
      wrap.className = 'dlg-wrap';
      wrap.innerHTML =
        '<div class="dlg"><div class="dlg-title"></div><div class="dlg-msg"></div>' +
        '<input type="text" spellcheck="false">' +
        '<div class="dlg-btns"><button data-ok tabindex="-1">OK</button><button data-cancel tabindex="-1">Cancel</button></div></div>';
      wrap.querySelector('.dlg-title').textContent = title;
      wrap.querySelector('.dlg-msg').textContent = label;
      const input = wrap.querySelector('input');
      document.body.appendChild(wrap);
      modalOpen++;
      input.focus();

      const finish = (ok) => {
        const text = ok ? input.value.trim() : null;
        wrap.remove();
        modalOpen--;
        resolve(text);
      };
      input.addEventListener('keydown', (e) => {
        if (e.key === 'Enter') finish(true);
        else if (e.key === 'Escape') finish(false);
      });
      wrap.querySelector('[data-ok]').addEventListener('click', () => finish(true));
      wrap.querySelector('[data-cancel]').addEventListener('click', () => finish(false));
    });
  }

  function info(title, text) {
    return new Promise((resolve) => {
      const wrap = document.createElement('div');
      wrap.className = 'dlg-wrap';
      wrap.innerHTML =
        '<div class="dlg info"><div class="dlg-title"></div><div class="dlg-msg"></div>' +
        '<div class="dlg-btns"><button tabindex="-1">OK</button></div></div>';
      wrap.querySelector('.dlg-title').textContent = title;
      wrap.querySelector('.dlg-msg').textContent = text;
      document.body.appendChild(wrap);
      modalOpen++;
      const finish = () => {
        document.removeEventListener('keydown', onKey, true);
        wrap.remove();
        modalOpen--;
        resolve();
      };
      const onKey = (e) => { if (e.key === 'Escape' || e.key === 'Enter') finish(); };
      document.addEventListener('keydown', onKey, true);
      wrap.querySelector('button').addEventListener('click', finish);
    });
  }

  // ------------------------------------------------------------- sidebar

  function toggleSidebar() {
    sidebarExpanded = !sidebarExpanded;
    if (sidebarExpanded) {
      ui.sidebar.style.display = '';
      void ui.sidebar.offsetWidth;                 // restart the width transition from 0
      ui.sidebar.classList.remove('collapsed');
    } else {
      ui.sidebar.classList.add('collapsed');
    }
  }
  ui.sidebar.addEventListener('transitionend', (e) => {
    if (e.propertyName === 'width' && !sidebarExpanded) ui.sidebar.style.display = 'none';
  });

  // ------------------------------------------------------------- window behaviour

  const iconBox = $('app-icon');
  iconBox.addEventListener('click', toggleSidebar);
  iconBox.addEventListener('contextmenu', (e) => showMainMenu(e.clientX, e.clientY));
  $('icon-img').addEventListener('error', (e) => {
    const dot = document.createElement('div');
    dot.className = 'dot';
    e.target.replaceWith(dot);
  });

  $('btn-min').addEventListener('click', () => appWindow.minimize());
  $('btn-close').addEventListener('click', async () => {
    P.active = false;
    audio.pause();
    await persist();
    appWindow.close();
  });
  $('btn-add').addEventListener('click', addPlaylist);
  $('btn-shuffle').addEventListener('click', toggleShuffle);
  $('btn-prev').addEventListener('click', playPrevious);
  $('btn-play').addEventListener('click', togglePlay);
  $('btn-next').addEventListener('click', playNext);
  $('btn-repeat').addEventListener('click', toggleRepeat);

  // Drag the window from any empty area (list, tree, slider, buttons and dialogs keep their own clicks).
  const NO_DRAG = 'button, input, #tree, #list, #slider, #app-icon, .menu, .dlg';
  document.addEventListener('mousedown', (e) => {
    if (e.target.closest('button')) e.preventDefault();       // buttons never take focus
    if (e.button === 0 && !e.target.closest(NO_DRAG) && !modalOpen) appWindow.startDragging();
  });
  document.addEventListener('contextmenu', (e) => e.preventDefault());

  // Keyboard shortcuts
  document.addEventListener('keydown', (e) => {
    if (modalOpen || e.ctrlKey || e.altKey || e.metaKey || e.target.closest('input')) return;
    const actions = {
      ' ': togglePlay,
      ArrowUp: () => adjustVolume(0.05),
      ArrowDown: () => adjustVolume(-0.05),
      ArrowLeft: () => seekBy(-10000),
      ArrowRight: () => seekBy(10000),
      '+': addPlaylist,
      '=': addPlaylist,
      s: toggleShuffle,
      S: toggleShuffle,
      r: toggleRepeat,
      R: toggleRepeat,
    };
    const action = actions[e.key];
    if (action) {
      e.preventDefault();
      action();
    }
  });

  document.addEventListener('visibilitychange', () => {
    viz.setActive(!document.hidden);                // don't burn CPU animating while minimized
    syncRainbowTimer();
  });

  // ------------------------------------------------------------- startup

  async function init() {
    const [saved, config] = await Promise.all([
      invoke('load_settings').catch(() => ({})),
      invoke('app_config').catch(() => cfg),
    ]);
    cfg = config;
    hasCookies = await invoke('cookies_status').catch(() => false);

    S.themeName = THEMES[saved.theme_name] ? saved.theme_name : 'Pink';
    S.theme = THEMES[S.themeName];
    S.bgAlpha = Number.isFinite(saved.bg_alpha) ? saved.bg_alpha : DEFAULT_ALPHA;
    S.playlists = saved.playlists && typeof saved.playlists === 'object' ? saved.playlists : {};
    S.repeat = !!saved.repeat;
    S.shuffle = !!saved.shuffle;
    S.discordOn = saved.discord_rpc !== false;
    audio.volume = clamp(Number.isFinite(saved.volume) ? saved.volume : 0.8, 0, 1);

    btn.shuffle.setActive(S.shuffle);
    btn.repeat.setActive(S.repeat);
    refreshVolumeLabel();
    applyTheme();
    viz.init();
    updateSidebarTree();
    syncRainbowTimer();
  }

  init();
})();
