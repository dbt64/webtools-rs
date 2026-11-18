export const PLUGIN_WINDOW_CHROME_CHANNELS = {
  getState: 'plugin-window-chrome:get-state',
  action: 'plugin-window-chrome:action',
  state: 'plugin-window-chrome:state',
  input: 'plugin-window-chrome:subinput',
  selectResult: 'plugin-window-chrome:select-result',
} as const

export type PluginWindowMode = 'attached' | 'detached'
export interface PluginSearchResultItem { id: string; title: string; description: string; icon: string }
export interface PluginSearchState {
  visible: boolean
  placeholder: string
  value: string
  loading: boolean
  error: string
  revision: number
  items: readonly PluginSearchResultItem[]
  focusToken: number
  valueToken: number
}
export type PluginSearchUiAction =
  | { type: 'show'; placeholder: string }
  | { type: 'hide' | 'get-text' | 'focus' | 'release-focus' }
  | { type: 'set-text'; text: string }
  | { type: 'results-update'; query: string; revision: number; items: readonly (Omit<PluginSearchResultItem, 'description' | 'icon'> & { description?: string; icon?: string })[]; loading: boolean; error?: string }
  | { type: 'results-clear'; query: string; revision: number }

export type PluginWindowAction =
  | 'toggle-menu'
  | 'close-menu'
  | 'toggle-detach'
  | 'about'
  | 'settings'
  | 'exit-background'
  | 'end-run'
  | 'pin'
  | 'minimize'
  | 'toggle-maximize'
  | 'close'

export interface PluginWindowChromeState {
  customWindow?: boolean
  title: string
  iconDataUrl: string
  mode: PluginWindowMode
  alwaysOnTop: boolean
  maximized: boolean
  loading: boolean
  themeCss: string
  search: PluginSearchState
}

export interface PluginWindowChromeApi {
  getState(): Promise<PluginWindowChromeState | null>
  perform(action: PluginWindowAction): Promise<boolean>
  onState(handler: (state: PluginWindowChromeState) => void): () => void
  setSearchInput(value: string): Promise<boolean>
  selectSearchResult(id: string): Promise<boolean>
}

declare global {
  interface Window { pluginWindowChrome: PluginWindowChromeApi }
}
