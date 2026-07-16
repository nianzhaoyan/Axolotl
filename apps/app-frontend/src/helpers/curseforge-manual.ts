export interface CurseForgeManualDownloadItem {
	projectId: number
	fileId: number
	fileName: string
	websiteUrl?: string
}

const STORAGE_KEY = 'axolotl.curseforge.manual-downloads.v1'

type ManualDownloadMap = Record<string, CurseForgeManualDownloadItem[]>

function readStore(): ManualDownloadMap {
	try {
		const raw = localStorage.getItem(STORAGE_KEY)
		if (!raw) return {}
		const parsed = JSON.parse(raw) as ManualDownloadMap
		return parsed && typeof parsed === 'object' ? parsed : {}
	} catch {
		return {}
	}
}

function writeStore(store: ManualDownloadMap) {
	localStorage.setItem(STORAGE_KEY, JSON.stringify(store))
}

export function getCurseForgeManualDownloads(instanceId: string): CurseForgeManualDownloadItem[] {
	return readStore()[instanceId] ?? []
}

export function setCurseForgeManualDownloads(
	instanceId: string,
	items: CurseForgeManualDownloadItem[],
) {
	const store = readStore()
	if (!items.length) {
		const { [instanceId]: _removed, ...rest } = store
		writeStore(rest)
		return
	}

	const deduped = new Map<string, CurseForgeManualDownloadItem>()
	for (const item of items) {
		deduped.set(`${item.projectId}:${item.fileId}`, item)
	}
	store[instanceId] = [...deduped.values()]
	writeStore(store)
}

export function clearCurseForgeManualDownloads(instanceId: string) {
	setCurseForgeManualDownloads(instanceId, [])
}

export function removeInstalledCurseForgeManualDownloads(
	instanceId: string,
	installedFileNames: string[],
) {
	const installed = new Set(installedFileNames.map((name) => name.toLowerCase()))
	const remaining = getCurseForgeManualDownloads(instanceId).filter(
		(item) => !installed.has(item.fileName.toLowerCase()),
	)
	setCurseForgeManualDownloads(instanceId, remaining)
	return remaining
}
