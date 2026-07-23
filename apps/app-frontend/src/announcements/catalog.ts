export type AnnouncementLocale = 'en-US' | 'zh-CN'

export type AnnouncementChangeType =
	| 'added'
	| 'changed'
	| 'deprecated'
	| 'removed'
	| 'fixed'
	| 'security'

export type LocalizedAnnouncementText = Readonly<Record<AnnouncementLocale, string>>

export type AnnouncementChange = LocalizedAnnouncementText

export type LauncherAnnouncement = {
	readonly id: string
	readonly version: string
	readonly publishedAt: string
	readonly title: LocalizedAnnouncementText
	readonly changes: Readonly<Partial<Record<AnnouncementChangeType, readonly AnnouncementChange[]>>>
	readonly notes?: LocalizedAnnouncementText
	readonly externalUrl?: string
}

export const ANNOUNCEMENT_CHANGE_TYPES: readonly AnnouncementChangeType[] = [
	'added',
	'changed',
	'deprecated',
	'removed',
	'fixed',
	'security',
]

export const launcherAnnouncements: readonly LauncherAnnouncement[] = [
	{
		id: 'launcher-1.4.0',
		version: '1.4.0',
		publishedAt: '2026-07-23',
		title: {
			'en-US': 'Axolotl Launcher 1.4.0',
			'zh-CN': 'Axolotl Launcher 1.4.0',
		},
		changes: {
			added: [
				{
					'en-US':
						'Added categorized update announcements after app updates and a permanent release history in settings.',
					'zh-CN': '新增应用更新后的分类公告弹窗，以及设置中的永久版本历史记录。',
				},
				{
					'en-US': 'Added a first-run onboarding guide that can also be replayed from settings.',
					'zh-CN': '新增首次使用引导，并支持从设置中重新播放。',
				},
			],
			changed: [
				{
					'en-US': 'Skipped-download warnings can now be collapsed.',
					'zh-CN': '跳过下载模组的警告窗口现在可以被收起。',
				},
				{
					'en-US': 'Launcher logs now rotate automatically at 10 MiB and keep up to five files.',
					'zh-CN': '启动器日志现按 10 MiB 自动轮转并最多保留 5 个文件。',
				},
				{
					'en-US':
						'Modrinth request logs now retain the target, source, retry count, and a redacted URL.',
					'zh-CN': 'Modrinth 请求日志现在保留目标、来源、重试次数和脱敏 URL。',
				},
				{
					'en-US': 'Large error log exports now use streaming compression to reduce memory usage.',
					'zh-CN': '错误日志导出现在使用流式压缩，降低大日志导出时的内存占用。',
				},
				{
					'en-US':
						'WARN and ERROR logs now rotate before the 30 MiB boundary without splitting individual events.',
					'zh-CN': 'WARN 和 ERROR 日志现在会在 30 MiB 边界内保持完整，轮转时不会拆分单个事件。',
				},
				{
					'en-US': 'Launcher logs older than three days are now removed automatically.',
					'zh-CN': '启动器日志创建超过三天后现在会自动删除。',
				},
			],
			fixed: [
				{
					'en-US': 'Fixed skipped mods remaining in the list after manually installing them.',
					'zh-CN': '修复手动安装跳过下载的模组后，已跳过模组列表不会更新的问题。',
				},
				{
					'en-US':
						'Fixed duplicate download events causing complete installation states to be logged repeatedly.',
					'zh-CN': '修复下载事件重复记录完整安装状态，导致启动器日志快速膨胀的问题。',
				},
				{
					'en-US':
						'Fixed the Fabric/Modrinth content page watcher repeatedly writing the same map and getting stuck loading.',
					'zh-CN':
						'修复 Fabric/Modrinth 实例内容页 watcher 重复写入相同 Map，触发递归更新并持续加载的问题。',
				},
			],
			security: [
				{
					'en-US': 'Temporary signatures in Modrinth request URLs are no longer written to logs.',
					'zh-CN': 'Modrinth 请求 URL 中的临时签名不再写入日志。',
				},
			],
		},
	},
]

export function getAnnouncementByVersion(version: string | null | undefined) {
	if (!version) return undefined
	return launcherAnnouncements.find((announcement) => announcement.version === version)
}

export function getAnnouncements(): readonly LauncherAnnouncement[] {
	return launcherAnnouncements
}

export function getAnnouncementById(id: string) {
	return launcherAnnouncements.find((announcement) => announcement.id === id)
}

export function getLocalizedAnnouncementText(
	text: LocalizedAnnouncementText,
	locale: string,
): string {
	return locale === 'zh-CN' ? text['zh-CN'] : text['en-US']
}
