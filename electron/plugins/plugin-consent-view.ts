import type { InstalledPluginCapability as PluginCapability, PluginVersionChangeKind } from '../../src/shared/plugin-contracts.ts'
import type { PluginConsent } from './permission-broker.ts'

const capabilityNames: Record<PluginCapability, string> = {
  'manager.page': '添加 Manager 页面',
  'plugin.config.read': '读取插件设置',
  'plugin.config.write': '保存插件设置',
  'plugin.storage.read': '读取插件私有数据（键值、文件和备份）',
  'plugin.storage.write': '保存插件私有数据（键值、文件和备份）',
  'external.open': '通过系统浏览器打开 HTTP(S) 链接',
  'clipboard.write': '写入剪贴板',
  'sharedAI.complete': '使用共享 AI',
  'clipboard.read': '仅在原生操作后接收一次剪贴板文本',
  'clipboard.current.read': '读取当前剪贴板',
  'network.domains': '访问声明的网络域名（当前不支持）',
  'plugin.window.manage': '创建并管理插件窗口',
  'system.file.select': '选择文件或文件夹',
  'system.path.open': '打开文件或文件夹',
  'system.notification': '发送系统通知',
  'system.input.paste': '向原应用粘贴文字',
  'system.browser.automation': '在可视浏览器中操作网页和读取内容（按插件隔离登录会话）',
  'system.media.transcode': '经选择保存位置后转换音视频（FFmpeg）',
  'system.media.image': '调整图片尺寸并转换 PNG/JPEG',
  'plugin.tasks': '经逐项确认后创建定时任务，在宿主托盘运行时执行',
  'system.screen.capture': '读取显示器信息并截取屏幕',
}

const changeNames: Record<PluginVersionChangeKind, string> = {
  'new-install': '新安装',
  'same-version-replacement': '同版本内容替换',
  upgrade: '升级',
  downgrade: '降级',
}

function capabilityList(values: PluginCapability[]): string {
  return values.length ? values.map(value => capabilityNames[value]).join('、') : '无'
}

/** Human-readable details for the Main-owned native consent prompt. */
export function formatPluginConsentDetail(request: PluginConsent): string {
  const lines = [`${request.name}（${request.pluginId}）`]
  if (request.manifestVersion === 3 && request.runtimeKind === 'node') {
    lines.push('本地代码信任：此插件会在当前 Windows 用户身份下运行 Node.js 代码，可使用 Node.js、npm 依赖、文件系统和子进程访问该用户本来可访问的资源。WebTools 不提供 OS 级隔离。')
    lines.push('WebTools 边界：此信任不会授予 Manager 私有 IPC、Shared AI 原始凭据、SecretStore 或 NativeHost 特权接口。')
  }
  const change = request.versionChange
  if (change) {
    lines.push(`当前版本：${change.currentVersion ?? '未安装'}`)
    lines.push(`即将导入：${change.incomingVersion}`)
    lines.push(`操作类型：${changeNames[change.kind]}`)
    lines.push(`新增申请能力：${capabilityList(change.addedCapabilities)}`)
    lines.push(`不再申请能力：${capabilityList(change.removedCapabilities)}`)
    if (change.reauthorizationRequired) lines.push('权限影响：此版本需要重新授权缺少的申请能力；确认导入不会自动授权，启用前需单独确认。')
    else if (change.currentVersion) lines.push('权限影响：仍申请且已授予的能力会保留；此版本不再申请的能力将自动撤销。')
    else lines.push('权限影响：新插件将保持停用；导入不会自动授予申请的能力。')
  } else {
    lines.push(`版本：${request.version}`)
  }
  if (request.capabilities?.length) lines.push(`本次请求的能力：${capabilityList(request.capabilities)}`)
  if (request.preview) lines.push(request.preview)
  return lines.join('\n')
}
