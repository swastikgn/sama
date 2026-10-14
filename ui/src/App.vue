<script setup>
import { computed, onMounted, onUnmounted, ref } from 'vue'

const backends = ref([])
const algorithm = ref('round_robin')
const appliedAlgorithm = ref('round_robin')
const firstLoad = ref(true)
const online = ref(true)
const theme = ref('system')
const addForm = ref({ address: '127.0.0.1:8006', weight: 1 })
const showAdd = ref(false)
const addError = ref('')
const weightDraft = ref({})
const weightError = ref({})
const deleteTarget = ref(null)
const saving = ref('')
const toasts = ref([])
let toastId = 0
let timer = null
let mediaQuery = null

const ALGOS = [
  { id: 'round_robin', name: 'Round Robin', desc: 'Connections take turns in order. Simple and fair.' },
  { id: 'weighted_round_robin', name: 'Weighted Round Robin', desc: 'Traffic share follows each origin\u2019s weight.' },
  { id: 'least_connections', name: 'Least Connections', desc: 'New connections go to the origin with the fewest active connections.' },
  { id: 'power_of_two_choices', name: 'Power of Two Choices', desc: 'Samples two origins at random and picks the less-loaded one.' },
  { id: 'random', name: 'Random', desc: 'Picks a healthy origin uniformly at random.' },
  { id: 'consistent_hash', name: 'Consistent Hash', desc: 'The same client IP always lands on the same origin.' },
]
const algoName = (id) => (ALGOS.find((a) => a.id === id) || { name: id }).name

/* ---------- theme ---------- */
function resolveTheme(mode) {
  if (mode === 'light') return 'light'
  if (mode === 'dark') return 'dark'
  return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'
}
function applyTheme() {
  document.documentElement.dataset.theme = resolveTheme(theme.value)
  try {
    localStorage.setItem('sama-theme', theme.value)
  } catch (e) { /* private mode */ }
}
function setTheme(mode) {
  theme.value = mode
  applyTheme()
}

/* ---------- toasts ---------- */
function pushToast(kind, title, msg = '') {
  const id = ++toastId
  toasts.value = [...toasts.value.slice(-2), { id, kind, title, msg }]
  setTimeout(() => dismissToast(id), 5000)
}
function dismissToast(id) {
  toasts.value = toasts.value.filter((t) => t.id !== id)
}
const toastIcon = (kind) =>
  kind === 'error'
    ? 'M12 8v5m0 3.5v.5M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0Z'
    : kind === 'success'
      ? 'M20 6 9 17l-5-5'
      : 'M12 8v5m0 3.5v.5M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20Z'

/* ---------- formatting ---------- */
function fmtNum(n) {
  return Number(n || 0).toLocaleString('en-US')
}
function fmtBytes(n) {
  n = Number(n || 0)
  if (n < 1024) return `${n} B`
  const units = ['KB', 'MB', 'GB', 'TB']
  let v = n / 1024
  let u = 0
  while (v >= 1024 && u < units.length - 1) { v /= 1024; u++ }
  return `${v >= 100 ? Math.round(v) : v.toFixed(1)} ${units[u]}`
}

/* ---------- backend reliability (only shown when something is wrong) ---------- */
function netIssues(b) {
  const parts = []
  if ((b.connect_failures || 0) > 0) parts.push(`${fmtNum(b.connect_failures)} failed dials`)
  if ((b.probe_failures || 0) > 0) parts.push(`${fmtNum(b.probe_failures)} failed probes`)
  return parts.length ? parts.join(' · ') : ''
}

/* ---------- api ---------- */
async function api(path, opts = {}) {
  const res = await fetch(path, {
    headers: { 'Content-Type': 'application/json' },
    ...opts,
  })
  const data = await res.json().catch(() => ({}))
  if (!res.ok) throw new Error(data.error || res.statusText)
  return data
}

async function refresh() {
  try {
    const [list, algo] = await Promise.all([
      api('/api/v1/backends'),
      api('/api/v1/algorithm'),
    ])
    backends.value = list
    online.value = true
    const prevApplied = appliedAlgorithm.value
    appliedAlgorithm.value = algo.algorithm
    if (firstLoad.value || algorithm.value === prevApplied) algorithm.value = algo.algorithm
    for (const b of list) {
      if (!(b.address in weightDraft.value)) weightDraft.value[b.address] = b.weight
    }
    for (const k of Object.keys(weightDraft.value)) {
      if (!list.some((b) => b.address === k)) delete weightDraft.value[k]
    }
  } catch (e) {
    online.value = false
    if (firstLoad.value) pushToast('error', 'Balancer unreachable', String(e.message || e))
  } finally {
    firstLoad.value = false
  }
}

/* ---------- derived ---------- */
const stats = computed(() => {
  const total = backends.value.length
  const healthy = backends.value.filter((b) => b.enabled && !b.draining && b.healthy).length
  const active = backends.value.reduce((n, b) => n + (b.active_connections || 0), 0)
  const served = backends.value.reduce((n, b) => n + (b.total_connections || 0), 0)
  return { total, healthy, active, served }
})
const algoDirty = computed(() => algorithm.value !== appliedAlgorithm.value)
const totalWeight = computed(() => backends.value.reduce((n, b) => n + (Number(b.weight) || 0), 0))
function shareOf(b) {
  if (!totalWeight.value) return 0
  return (Number(b.weight) || 0) / totalWeight.value
}

function statusOf(b) {
  const pill = 'padding:3px 10px;border-radius:999px;border:1px solid transparent;'
  if (!b.enabled) return { label: 'Disabled', dot: 'background:#9ca3af;', pill: pill + 'background:var(--surface-2);color:var(--muted);border-color:var(--line);' }
  if (b.draining) return { label: 'Draining', dot: 'background:var(--warn);', pill: pill + 'background:var(--warn-bg);color:var(--warn);border-color:color-mix(in srgb, var(--warn) 35%, transparent);' }
  if (b.healthy) return { label: 'Healthy', dot: 'background:var(--good);', pill: pill + 'background:var(--good-bg);color:var(--good);border-color:color-mix(in srgb, var(--good) 35%, transparent);' }
  return { label: 'Down', dot: 'background:var(--bad);', pill: pill + 'background:var(--bad-bg);color:var(--bad);border-color:color-mix(in srgb, var(--bad) 40%, transparent);' }
}

/* ---------- actions ---------- */
function validSocketAddr(v) {
  const m = /^(.+):(\d+)$/.exec(v.trim())
  if (!m) return false
  const port = Number(m[2])
  return Number.isInteger(port) && port > 0 && port < 65536
}
function clampWeight(w) {
  return Math.min(1000, Math.max(1, Math.round(Number(w) || 1)))
}
function bumpWeight(b, d) {
  weightDraft.value[b.address] = clampWeight(Number(weightDraft.value[b.address] ?? b.weight) + d)
  delete weightError.value[b.address]
}
function bumpAddWeight(d) {
  addForm.value.weight = clampWeight(Number(addForm.value.weight || 1) + d)
  addError.value = ''
}

function openAdd() {
  addError.value = ''
  addForm.value = { address: '127.0.0.1:8006', weight: 1 }
  showAdd.value = true
}

async function addBackend() {
  const addr = addForm.value.address.trim()
  const w = Number(addForm.value.weight)
  if (!validSocketAddr(addr)) {
    addError.value = 'Use host:port — e.g. 127.0.0.1:8006.'
    return
  }
  if (!Number.isInteger(w) || w < 1 || w > 1000) {
    addError.value = 'Weight must be a whole number between 1 and 1000.'
    return
  }
  addError.value = ''
  try {
    await api('/api/v1/backends', {
      method: 'POST',
      body: JSON.stringify({ address: addr, weight: w, enabled: true }),
    })
    pushToast('success', 'Origin added', `${addr} joins rotation once health checks pass.`)
    showAdd.value = false
    await refresh()
  } catch (e) {
    pushToast('error', 'Could not add origin', String(e.message || e))
  }
}

async function patch(b, body) {
  saving.value = b.address
  try {
    await api(`/api/v1/backends/${encodeURIComponent(b.address)}`, {
      method: 'PATCH',
      body: JSON.stringify(body),
    })
    await refresh()
  } catch (e) {
    pushToast('error', `Could not update ${b.address}`, String(e.message || e))
  } finally {
    saving.value = ''
  }
}

async function saveWeight(b) {
  const w = Number(weightDraft.value[b.address])
  if (!Number.isInteger(w) || w < 1 || w > 1000) {
    weightError.value[b.address] = 'Use a whole number 1–1000.'
    return
  }
  delete weightError.value[b.address]
  await patch(b, { weight: w })
  pushToast('success', 'Weight updated', `${b.address} now has weight ${w}.`)
}

function askDelete(b) {
  if (b.active_connections > 0) {
    pushToast('error', `Can't delete ${b.address} yet`, 'Drain it first and wait for active connections to reach 0.')
    return
  }
  deleteTarget.value = b
}

async function removeBackend() {
  const b = deleteTarget.value
  if (!b) return
  deleteTarget.value = null
  try {
    await api(`/api/v1/backends/${encodeURIComponent(b.address)}`, { method: 'DELETE' })
    pushToast('success', 'Origin deleted', b.address)
    await refresh()
  } catch (e) {
    pushToast('error', `Could not delete ${b.address}`, String(e.message || e))
  }
}

async function applyAlgorithm() {
  try {
    await api('/api/v1/algorithm', { method: 'PUT', body: JSON.stringify({ algorithm: algorithm.value }) })
    appliedAlgorithm.value = algorithm.value
    pushToast('success', 'Traffic policy deployed', `New connections now use ${algoName(algorithm.value)}.`)
    await refresh()
  } catch (e) {
    pushToast('error', 'Could not switch routing', String(e.message || e))
  }
}

function onKey(e) {
  if (e.key === 'Escape') {
    deleteTarget.value = null
    showAdd.value = false
  }
}
function onSystemTheme() {
  if (theme.value === 'system') applyTheme()
}
function onVisibility() {
  if (document.hidden) clearInterval(timer)
  else {
    clearInterval(timer)
    timer = setInterval(refresh, 2000)
  }
}

onMounted(() => {
  try {
    theme.value = localStorage.getItem('sama-theme') || 'system'
  } catch (e) { theme.value = 'system' }
  applyTheme()
  mediaQuery = window.matchMedia('(prefers-color-scheme: dark)')
  mediaQuery.addEventListener('change', onSystemTheme)
  refresh()
  timer = setInterval(refresh, 2000)
  window.addEventListener('keydown', onKey)
  document.addEventListener('visibilitychange', onVisibility)
})
onUnmounted(() => {
  clearInterval(timer)
  window.removeEventListener('keydown', onKey)
  document.removeEventListener('visibilitychange', onVisibility)
  if (mediaQuery) mediaQuery.removeEventListener('change', onSystemTheme)
})
</script>

<template>
  <div class="min-h-screen" style="background: var(--bg); color: var(--ink)">
    <!-- top nav -->
    <header class="cf-nav-wrap">
      <div class="mx-auto flex max-w-6xl items-center gap-3 px-4 py-3 sm:px-6">
        <a class="cf-logo" href="#" @click.prevent>
          <span class="cf-logo-mark" aria-hidden="true">
            <svg id="sama-logo" viewBox="0 0 48 48">
              <g class="logo-beam"><rect x="5" y="23" width="38" height="6" rx="3" fill="#fff" /></g>
              <circle class="logo-orb" cx="24" cy="18" r="5" fill="#141419" />
            </svg>
          </span>
          <span>
            <span class="cf-logo-name"><span lang="kn">ಸಮ</span> SAMA</span>
            <span class="cf-logo-sub">Load Balancer</span>
          </span>
        </a>
        <div class="ml-auto flex items-center gap-2.5">
          <div class="cf-theme" role="group" aria-label="Appearance">
            <button :class="theme === 'light' ? 'on' : ''" @click="setTheme('light')" title="Light mode" aria-label="Light mode">
              <svg viewBox="0 0 24 24" class="h-4 w-4" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="12" cy="12" r="4" /><path d="M12 2v2m0 16v2M4.9 4.9l1.4 1.4m11.4 11.4 1.4 1.4M2 12h2m16 0h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" /></svg>
            </button>
            <button :class="theme === 'system' ? 'on' : ''" @click="setTheme('system')" title="System" aria-label="System theme">
              <svg viewBox="0 0 24 24" class="h-4 w-4" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><rect x="3" y="4" width="18" height="12" rx="2" /><path d="M9 20h6M12 16v4" /></svg>
            </button>
            <button :class="theme === 'dark' ? 'on' : ''" @click="setTheme('dark')" title="Dark mode" aria-label="Dark mode">
              <svg viewBox="0 0 24 24" class="h-4 w-4" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8Z" /></svg>
            </button>
          </div>
          <span
            class="cf-pill"
            :class="online ? 'live' : ''"
            :style="online
              ? 'background: var(--good-bg); color: var(--good);'
              : 'background: var(--bad-bg); color: var(--bad);'">
            <span class="dot"></span>
            {{ online ? 'Operational' : 'Offline' }}
          </span>
        </div>
      </div>
      <div v-if="!online" class="border-t px-4 py-2 text-center text-xs font-semibold sm:px-6" style="border-color: var(--line); background: var(--bad-bg); color: var(--bad)">
        Connection to the balancer lost — showing last known state.
        <button @click="refresh()" class="cf-action ml-1">Retry now</button>
      </div>
    </header>

    <main class="mx-auto max-w-6xl px-4 pb-20 sm:px-6">
      <!-- overview: three separate stat cards -->
      <section aria-label="Overview" class="mt-8 grid gap-3 sm:grid-cols-3">
        <div class="cf-card p-5 sm:p-6">
          <p class="cf-stat-num">{{ stats.total }}</p>
          <p class="mt-1 text-sm font-semibold" style="color: var(--muted)">Origins</p>
          <p class="mt-0.5 text-[13px]" style="color: var(--muted)">{{ stats.healthy }} healthy</p>
        </div>
        <div class="cf-card p-5 sm:p-6">
          <div class="flex items-center gap-2">
            <p class="cf-stat-num">{{ fmtNum(stats.active) }}</p>
            <span v-if="stats.active > 0" class="inline-block h-2 w-2 rounded-full" style="background: var(--accent)" aria-hidden="true"></span>
          </div>
          <p class="mt-1 text-sm font-semibold" style="color: var(--muted)">Active connections</p>
          <p class="mt-0.5 text-[13px]" style="color: var(--muted)">live right now</p>
        </div>
        <div class="cf-card p-5 sm:p-6">
          <p class="cf-stat-num">{{ fmtNum(stats.served) }}</p>
          <p class="mt-1 text-sm font-semibold" style="color: var(--muted)">Connections served</p>
          <p class="mt-0.5 text-[13px]" style="color: var(--muted)">all time</p>
        </div>
      </section>

      <!-- routing -->
      <section aria-label="Traffic policy" class="cf-card mt-4 p-5 sm:p-6">
        <h2 class="text-lg font-extrabold tracking-tight">Traffic policy</h2>
        <div class="mt-3 grid gap-2 sm:grid-cols-2 lg:grid-cols-3">
          <button
            v-for="a in ALGOS" :key="a.id" @click="algorithm = a.id"
            class="algo border px-3.5 py-3 text-left transition-colors"
            :aria-pressed="algorithm === a.id"
            style="border-color: var(--line); background: var(--surface);">
            <span class="flex items-center gap-2 text-[15px] font-bold" style="letter-spacing: -0.01em">
              <span class="inline-block h-2 w-2 shrink-0 rounded-full" :style="{ background: algorithm === a.id ? 'var(--accent)' : 'var(--line)' }"></span>
              {{ a.name }}
              <span class="ml-auto flex shrink-0 items-center gap-1.5">
                <span
                  v-if="appliedAlgorithm === a.id"
                  class="rounded-full px-2 py-px text-[11px] font-bold"
                  style="background: var(--good-bg); color: var(--good)">Live</span>
                <svg viewBox="0 0 24 24" class="h-5 w-5" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" :style="{ color: 'var(--accent)', opacity: algorithm === a.id ? 1 : 0 }"><path d="M20 6 9 17l-5-5" /></svg>
              </span>
            </span>
            <span class="mt-1 block text-[13px] leading-relaxed" style="color: var(--muted)">{{ a.desc }}</span>
          </button>
        </div>
        <div class="mt-3 flex flex-wrap items-center gap-2">
          <button @click="applyAlgorithm" :disabled="!algoDirty" class="cf-btn cf-btn-primary px-5 py-2 text-sm">
            Apply changes</button>
          <button v-if="algoDirty" @click="algorithm = appliedAlgorithm" class="cf-btn cf-btn-secondary px-4 py-2 text-sm">Cancel</button>
        </div>
      </section>

      <!-- servers -->
      <section aria-label="Origin servers" class="mt-10">
        <div class="flex items-center gap-3">
          <h2 class="mr-auto text-xl font-extrabold tracking-tight">Origin servers · {{ backends.length }}</h2>
          <button @click="openAdd()" class="cf-btn cf-btn-primary px-4 py-2 text-sm">+ Add origin</button>
        </div>

        <div class="cf-card mt-4 overflow-hidden">
          <div v-if="firstLoad" class="space-y-2 p-5">
            <div v-for="i in 3" :key="i" class="skeleton h-14 rounded-lg"></div>
          </div>

          <div v-else-if="!backends.length" class="px-6 py-14 text-center">
            <p class="text-lg font-extrabold tracking-tight">No origins yet</p>
            <p class="mx-auto mt-1 max-w-sm text-sm" style="color: var(--muted)">Add your first origin server — it joins rotation automatically once health checks pass.</p>
            <button @click="openAdd()" class="cf-btn cf-btn-primary mx-auto mt-5 px-5 py-2.5 text-sm">+ Add your first origin</button>
          </div>

          <div v-else class="overflow-x-auto">
            <table class="cf-table">
              <thead>
                <tr>
                  <th>Status</th>
                  <th>Origin</th>
                  <th>Weight</th>
                  <th>Active</th>
                  <th style="text-align: right">Actions</th>
                </tr>
              </thead>
              <tbody>
                <tr v-for="b in backends" :key="b.address">
                  <td>
                    <span class="cf-badge" :style="statusOf(b).pill">
                      <span class="dot" :style="statusOf(b).dot"></span>{{ statusOf(b).label }}
                    </span>
                  </td>
                  <td>
                    <code class="addr-chip">{{ b.address }}</code>
                    <p class="tabular mt-0.5 text-xs" style="color: var(--muted)">{{ fmtNum(b.total_connections || 0) }} served · ↑ {{ fmtBytes(b.bytes_in || 0) }} · ↓ {{ fmtBytes(b.bytes_out || 0) }}</p>
                    <p v-if="netIssues(b)" class="tabular mt-0.5 text-xs font-semibold" style="color: var(--warn)">{{ netIssues(b) }}</p>
                  </td>
                  <td>
                    <div class="flex items-center gap-2">
                      <div class="cf-input flex items-center">
                        <button @click="bumpWeight(b, -1)" :disabled="Number(weightDraft[b.address] ?? b.weight) <= 1"
                          class="px-2 py-1 text-sm font-bold opacity-60 hover:opacity-100 disabled:opacity-25" aria-label="Decrease weight">−</button>
                        <input v-model.number="weightDraft[b.address]" @change="weightDraft[b.address] = clampWeight(weightDraft.value[b.address])"
                          type="number" min="1" max="1000"
                          class="cf-input no-spinner tabular w-12 border-0 px-1 py-1 text-center text-sm font-semibold" style="box-shadow: none" aria-label="Weight" />
                        <button @click="bumpWeight(b, 1)" :disabled="Number(weightDraft[b.address] ?? b.weight) >= 1000"
                          class="px-2 py-1 text-sm font-bold opacity-60 hover:opacity-100 disabled:opacity-25" aria-label="Increase weight">+</button>
                      </div>
                      <button @click="saveWeight(b)" :disabled="Number(weightDraft[b.address]) === b.weight"
                        class="cf-btn cf-btn-primary cf-btn-sm">Save</button>
                    </div>
                    <div class="mt-2 flex items-center gap-2">
                      <div class="share-track w-24" aria-hidden="true"><div class="share-fill" :style="{ width: (shareOf(b) * 100) + '%' }"></div></div>
                      <span class="tabular text-[11px] font-bold" style="color: var(--muted)">{{ Math.round(shareOf(b) * 100) }}%</span>
                    </div>
                    <p v-if="weightError[b.address]" role="alert" class="mt-1 text-xs font-semibold" style="color: var(--bad)">{{ weightError[b.address] }}</p>
                  </td>
                  <td><span class="load-pill" :class="(b.active_connections || 0) > 0 ? 'hot' : ''">{{ fmtNum(b.active_connections) }}</span></td>
                  <td style="text-align: right">
                    <div class="flex justify-end gap-1.5">
                      <button @click="patch(b, { enabled: !b.enabled })"
                        class="cf-btn cf-btn-secondary cf-btn-sm w-20 text-center"
                        :title="b.enabled ? 'Stop sending any traffic here' : 'Let this server take traffic again'">
                        {{ b.enabled ? 'Disable' : 'Enable' }}</button>
                      <button @click="patch(b, { draining: !b.draining })"
                        class="cf-btn cf-btn-sm w-20 text-center"
                        :class="b.draining ? 'cf-btn-warn' : 'cf-btn-secondary'"
                        :title="b.draining ? 'Let it take new traffic again' : 'Stop new traffic, let existing finish'">
                        {{ b.draining ? 'Undrain' : 'Drain' }}</button>
                      <button @click="askDelete(b)" :disabled="b.active_connections > 0"
                        :title="b.active_connections > 0 ? 'Drain first — active must reach 0' : 'Delete this server'"
                        class="cf-btn cf-btn-sm cf-btn-danger-outline">Delete</button>
                    </div>
                    <p v-if="saving === b.address" class="mt-1 text-[11px]" style="color: var(--muted)">Saving…</p>
                  </td>
                </tr>
              </tbody>
            </table>
          </div>
        </div>
      </section>

    </main>

    <!-- add dialog -->
    <div v-if="showAdd" class="fade-in fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4" @click.self="showAdd = false">
      <div class="modal-in cf-card w-full max-w-md p-6 sm:p-8" role="dialog" aria-modal="true" aria-label="Add origin">
        <h3 class="text-xl font-extrabold tracking-tight">Add origin server</h3>
        <p class="mt-1 text-sm" style="color: var(--muted)">Takes traffic once health checks pass.</p>
        <form @submit.prevent="addBackend" class="mt-5 space-y-4">
          <label class="flex flex-col gap-1.5 text-xs font-bold" style="color: var(--muted)">ADDRESS
            <input v-model="addForm.address" @input="addError = ''" spellcheck="false" placeholder="127.0.0.1:8006" required autofocus
              class="cf-input px-3.5 py-2.5 font-mono text-sm font-semibold" />
          </label>
          <label class="flex flex-col gap-1.5 text-xs font-bold" style="color: var(--muted)">WEIGHT
            <div class="cf-input flex items-center">
              <button type="button" @click="bumpAddWeight(-1)" :disabled="Number(addForm.weight || 1) <= 1"
                class="px-3 py-2 text-sm font-bold opacity-60 hover:opacity-100 disabled:opacity-25" aria-label="Decrease weight">−</button>
              <input v-model.number="addForm.weight" @input="addError = ''" @change="addForm.weight = clampWeight(addForm.weight)" type="number" min="1" max="1000" required
                class="cf-input no-spinner w-full border-0 px-1 py-2 text-center text-sm font-semibold" style="box-shadow: none" />
              <button type="button" @click="bumpAddWeight(1)" :disabled="Number(addForm.weight || 1) >= 1000"
                class="px-3 py-2 text-sm font-bold opacity-60 hover:opacity-100 disabled:opacity-25" aria-label="Increase weight">+</button>
            </div>
          </label>
          <p v-if="addError" role="alert" class="rounded-lg px-3.5 py-2.5 text-[13px] font-semibold" style="background: var(--bad-bg); color: var(--bad)">{{ addError }}</p>
          <div class="flex gap-2 pt-1">
            <button type="button" @click="showAdd = false" class="cf-btn cf-btn-secondary flex-1 px-4 py-2.5 text-sm">Cancel</button>
            <button type="submit" class="cf-btn cf-btn-primary flex-1 px-4 py-2.5 text-sm">Add origin</button>
          </div>
        </form>
      </div>
    </div>

    <!-- delete dialog -->
    <div v-if="deleteTarget" class="fade-in fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4" @click.self="deleteTarget = null">
      <div class="modal-in cf-card w-full max-w-md p-6 sm:p-8" role="alertdialog" aria-modal="true" aria-label="Confirm deletion">
        <h3 class="text-xl font-extrabold tracking-tight">Delete this origin?</h3>
        <code class="mt-3 inline-block rounded-md px-2.5 py-1.5 font-mono text-[13px] font-bold" style="background: var(--surface-2); border: 1px solid var(--line)">{{ deleteTarget.address }}</code>
        <p class="mt-3 text-sm leading-relaxed" style="color: var(--muted)">It stops receiving traffic immediately. You can re-add it any time — nothing else is affected.</p>
        <div class="mt-6 flex gap-2">
          <button @click="deleteTarget = null" class="cf-btn cf-btn-secondary flex-1 px-4 py-2.5 text-sm">Keep it</button>
          <button @click="removeBackend()" class="cf-btn cf-btn-danger flex-1 px-4 py-2.5 text-sm">Yes, delete</button>
        </div>
      </div>
    </div>

    <!-- toasts -->
    <div aria-live="polite" class="cf-toasts">
      <div v-for="t in toasts" :key="t.id" class="toast" :class="'toast-' + t.kind" role="status">
        <span class="toast-icon">
          <svg viewBox="0 0 24 24" class="h-[18px] w-[18px]" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path :d="toastIcon(t.kind)" /></svg>
        </span>
        <div class="toast-body">
          <p class="toast-title">{{ t.title }}</p>
          <p v-if="t.msg" class="toast-msg">{{ t.msg }}</p>
        </div>
        <button @click="dismissToast(t.id)" class="toast-x" aria-label="Dismiss notification">✕</button>
        <span class="toast-bar"></span>
      </div>
    </div>
  </div>
</template>
