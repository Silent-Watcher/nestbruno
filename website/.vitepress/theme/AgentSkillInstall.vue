<script setup>
import { ref, onUnmounted } from 'vue'

// The single install command, identical everywhere it is shown.
const command = 'npx skills add Silent-Watcher/nestbruno --skill nestbruno'

const commandEl = ref(null)
const copied = ref(false)
const failed = ref(false)

let resetTimer = null

function scheduleReset() {
  if (resetTimer !== null) clearTimeout(resetTimer)
  resetTimer = setTimeout(() => {
    copied.value = false
    failed.value = false
    resetTimer = null
  }, 2500)
}

onUnmounted(() => {
  if (resetTimer !== null) clearTimeout(resetTimer)
})

// Deprecated API, used only as a fallback when the Clipboard API is
// unavailable or the permission was denied. Runs inside the click handler.
function copyViaExecCommand() {
  const area = document.createElement('textarea')
  area.value = command
  area.setAttribute('readonly', '')
  area.style.position = 'fixed'
  area.style.opacity = '0'
  document.body.appendChild(area)
  area.select()
  const succeeded = document.execCommand('copy')
  document.body.removeChild(area)
  return succeeded
}

// Last resort: select the visible command so the user can copy it manually.
function selectVisibleCommand() {
  const selection = window.getSelection()
  const range = document.createRange()
  range.selectNodeContents(commandEl.value)
  selection.removeAllRanges()
  selection.addRange(range)
}

async function copyCommand() {
  try {
    if (!navigator.clipboard) throw new Error('Clipboard API unavailable')
    await navigator.clipboard.writeText(command)
    copied.value = true
    failed.value = false
  } catch {
    if (copyViaExecCommand()) {
      copied.value = true
      failed.value = false
    } else {
      copied.value = false
      failed.value = true
      selectVisibleCommand()
    }
  }
  scheduleReset()
}
</script>

<template>
  <div class="nb-skill-install">
    <code ref="commandEl" class="nb-skill-command" tabindex="0">{{ command }}</code>
    <button
      type="button"
      class="nb-skill-copy"
      :class="{ 'is-copied': copied, 'is-failed': failed }"
      @click="copyCommand"
    >
      {{ copied ? 'Copied ✓' : 'Copy install command' }}
    </button>
    <span class="nb-sr-status" aria-live="polite" role="status">
      {{
        copied
          ? 'Copied to clipboard.'
          : failed
            ? 'Automatic copy failed — the command is selected; copy it manually.'
            : ''
      }}
    </span>
  </div>
</template>

<style scoped>
.nb-skill-install {
  display: flex;
  flex-wrap: wrap;
  align-items: stretch;
  gap: 12px;
  margin: 16px 0;
}

.nb-skill-command {
  flex: 1 1 260px;
  min-width: 0;
  margin: 0;
  padding: 10px 14px;
  background: var(--vp-c-bg-elv);
  border: 1px solid var(--vp-c-divider);
  border-radius: 10px;
  font-family: var(--vp-font-family-mono, ui-monospace, monospace);
  font-size: 0.9em;
  color: var(--vp-c-text-1);
  overflow-x: auto;
  white-space: nowrap;
  user-select: all;
  outline: none;
}

.nb-skill-command:focus-visible {
  border-color: var(--vp-c-brand-1);
}

.nb-skill-copy {
  flex: 0 0 auto;
  padding: 10px 18px;
  border-radius: 10px;
  border: 1px solid var(--vp-c-brand-1);
  background: var(--vp-c-brand-1);
  color: var(--vp-button-brand-text, #fff);
  font-size: 0.9em;
  font-weight: 600;
  cursor: pointer;
  transition: background-color 0.15s ease;
  white-space: nowrap;
}

.nb-skill-copy:hover {
  background: var(--vp-c-brand-2);
  border-color: var(--vp-c-brand-2);
}

.nb-skill-copy.is-copied {
  background: var(--vp-c-brand-soft);
  border-color: var(--vp-c-brand-1);
  color: var(--vp-c-brand-1);
}

.nb-skill-copy.is-failed {
  background: transparent;
  border-color: var(--vp-c-divider);
  color: var(--vp-c-text-1);
  cursor: default;
}

/* Visible only to assistive technology; announces copy results. */
.nb-sr-status {
  position: absolute;
  width: 1px;
  height: 1px;
  margin: -1px;
  padding: 0;
  overflow: hidden;
  clip: rect(0 0 0 0);
  white-space: nowrap;
  border: 0;
}
</style>
