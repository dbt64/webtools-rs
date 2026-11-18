import type { PluginWindowAction, PluginWindowChromeState } from '../plugins/plugin-window-chrome-contract.ts'

const titlebar = document.querySelector<HTMLElement>('#plugin-window-titlebar')!
const title = document.querySelector<HTMLElement>('#plugin-window-title')!
const icon = document.querySelector<HTMLImageElement>('#plugin-window-icon')!
const rightIcon = document.querySelector<HTMLImageElement>('#plugin-window-right-icon')!
const loadingIcon = document.querySelector<HTMLImageElement>('#plugin-window-loading-icon')!
const loadingState = document.querySelector<HTMLElement>('#plugin-window-loading')!
const root = document.querySelector<HTMLElement>('#plugin-window-root')!
const maximizeButton = document.querySelector<HTMLButtonElement>('.maximize-button')!
const maximizeIcon = maximizeButton.querySelector<SVGElement>('.maximize-icon')!
const restoreIcon = maximizeButton.querySelector<SVGElement>('.restore-icon')!
const searchSurface = document.querySelector<HTMLElement>('#plugin-window-search')!
const searchInput = document.querySelector<HTMLInputElement>('#plugin-window-search-input')!
const searchStatus = document.querySelector<HTMLElement>('#plugin-window-search-status')!
const searchResults = document.querySelector<HTMLElement>('#plugin-window-search-results')!
const removeStateListener = window.pluginWindowChrome.onState(applyState)
let lastFocusToken = 0
let lastSearchRevision = 0
let lastSearchValueToken = -1
let selectedResultIndex = 0
let composing = false

function applyState(state: PluginWindowChromeState): void {
  title.textContent = state.title
  icon.src = state.iconDataUrl
  icon.hidden = !state.iconDataUrl
  rightIcon.src = state.iconDataUrl
  rightIcon.hidden = state.mode !== 'attached' || !state.iconDataUrl
  loadingIcon.src = state.iconDataUrl
  loadingIcon.hidden = !state.iconDataUrl
  loadingState.hidden = !state.loading
  root.dataset.mode = state.mode
  root.dataset.customWindow = String(Boolean(state.customWindow))
  document.querySelector<HTMLButtonElement>('.menu-trigger')!.hidden = Boolean(state.customWindow)
  root.dataset.loading = String(state.loading)
  root.dataset.pinned = String(state.alwaysOnTop)
  root.dataset.maximized = String(state.maximized)
  root.dataset.theme = /--webtools-color-scheme:\s*dark/.test(state.themeCss) ? 'dark' : 'light'
  document.querySelector<HTMLButtonElement>('.menu-trigger')!.textContent = state.mode === 'attached' ? '⋮' : '☰'
  let style = document.querySelector<HTMLStyleElement>('#plugin-window-theme')
  if (!style) { style = document.createElement('style'); style.id = 'plugin-window-theme'; document.head.append(style) }
  style.textContent = state.themeCss
  document.querySelectorAll<HTMLElement>('.detached-only').forEach(element => { element.hidden = state.mode !== 'detached' })
  document.querySelector<HTMLElement>('.attached-close')!.hidden = state.mode === 'detached'
  const pinButton = document.querySelector<HTMLButtonElement>('.pin-button')!
  pinButton.setAttribute('aria-label', state.alwaysOnTop ? '取消置顶' : '置顶')
  pinButton.title = state.alwaysOnTop ? '取消置顶' : '置顶'
  pinButton.setAttribute('aria-pressed', String(state.alwaysOnTop))
  const activeMaximizeIcon = state.maximized ? restoreIcon : maximizeIcon
  activeMaximizeIcon.removeAttribute('hidden')
  maximizeButton.replaceChildren(activeMaximizeIcon)
  maximizeButton.setAttribute('aria-label', state.maximized ? '还原' : '最大化')
  maximizeButton.title = state.maximized ? '还原' : '最大化'
  applySearchState(state.search)
}

function applySearchState(state: PluginWindowChromeState['search']): void {
  searchSurface.hidden = !state.visible
  if (!state.visible) return
  searchSurface.dataset.loading = String(state.loading)
  if (state.valueToken !== lastSearchValueToken) {
    lastSearchValueToken = state.valueToken
    if (!composing && searchInput.value !== state.value) searchInput.value = state.value
  }
  searchInput.placeholder = state.placeholder
  if (state.revision !== lastSearchRevision) { lastSearchRevision = state.revision; selectedResultIndex = 0 }
  if (state.focusToken !== lastFocusToken) {
    lastFocusToken = state.focusToken
    requestAnimationFrame(() => { if (!searchSurface.hidden) searchInput.focus({ preventScroll: true }) })
  }
  searchStatus.hidden = !(state.loading || state.error || !state.items.length)
  searchStatus.textContent = state.loading ? '正在搜索…' : state.error || (!state.value ? '输入内容以搜索' : state.items.length ? '' : '没有匹配结果')
  searchResults.replaceChildren()
  selectedResultIndex = Math.min(selectedResultIndex, Math.max(0, state.items.length - 1))
  state.items.forEach((item, index) => {
    const button = document.createElement('button')
    button.type = 'button'
    button.className = 'plugin-search-result'
    button.setAttribute('role', 'option')
    button.setAttribute('aria-selected', String(index === selectedResultIndex))
    button.dataset.resultId = item.id
    const image = document.createElement('img')
    image.className = 'plugin-search-result-icon'
    image.alt = ''
    image.hidden = !item.icon
    if (item.icon) image.src = item.icon
    const copy = document.createElement('span')
    copy.className = 'plugin-search-result-copy'
    const resultTitle = document.createElement('span')
    resultTitle.className = 'plugin-search-result-title'
    resultTitle.textContent = item.title
    copy.append(resultTitle)
    if (item.description) {
      const description = document.createElement('span')
      description.className = 'plugin-search-result-description'
      description.textContent = item.description
      copy.append(description)
    }
    button.append(image, copy)
    button.addEventListener('click', () => { void window.pluginWindowChrome.selectSearchResult(item.id) })
    button.addEventListener('pointermove', () => {
      selectedResultIndex = index
      for (const [position, element] of [...searchResults.children].entries()) element.setAttribute('aria-selected', String(position === index))
    })
    searchResults.append(button)
  })
}

function publishSearchText(): void {
  const encoder = new TextEncoder()
  let byteLength = 0
  let bounded = ''
  for (const character of searchInput.value) {
    const characterBytes = encoder.encode(character).length
    if (byteLength + characterBytes > 1024) break
    byteLength += characterBytes
    bounded += character
  }
  if (bounded !== searchInput.value) searchInput.value = bounded
  void window.pluginWindowChrome.setSearchInput(searchInput.value)
}

searchInput.addEventListener('compositionstart', () => { composing = true })
searchInput.addEventListener('compositionend', () => { composing = false; publishSearchText() })
searchInput.addEventListener('input', event => { if (!composing && !(event as InputEvent).isComposing) publishSearchText() })
searchInput.addEventListener('keydown', event => {
  if (event.isComposing || composing) return
  if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    const count = searchResults.children.length
    if (!count) return
    event.preventDefault()
    selectedResultIndex = (selectedResultIndex + (event.key === 'ArrowDown' ? 1 : count - 1)) % count
    for (const [position, element] of [...searchResults.children].entries()) {
      element.setAttribute('aria-selected', String(position === selectedResultIndex))
    }
  } else if (event.key === 'Enter') {
    const selected = searchResults.children[selectedResultIndex] as HTMLButtonElement | undefined
    if (selected?.dataset.resultId) { event.preventDefault(); void window.pluginWindowChrome.selectSearchResult(selected.dataset.resultId) }
  }
})

async function perform(action: PluginWindowAction): Promise<void> {
  await window.pluginWindowChrome.perform(action).catch(() => false)
}

titlebar.addEventListener('click', event => {
  const target = event.target instanceof Element ? event.target.closest<HTMLElement>('[data-action]') : null
  const action = target?.dataset.action as PluginWindowAction | undefined
  if (action) { event.preventDefault(); event.stopPropagation(); void perform(action); return }
  if (event.target instanceof Element && event.target.closest('.menu-trigger')) {
    event.preventDefault(); event.stopPropagation(); void perform('toggle-menu')
    return
  }
})

document.addEventListener('pointerdown', event => {
  if (event.target instanceof Element && event.target.closest('#plugin-window-titlebar')
    && !event.target.closest('.menu-trigger') && !event.target.closest('[data-action]')) void perform('close-menu')
})

void window.pluginWindowChrome.getState().then(state => { if (state) applyState(state) })
window.addEventListener('beforeunload', removeStateListener, { once: true })
