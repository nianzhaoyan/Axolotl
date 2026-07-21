<script setup>
import {
	BoxIcon,
	DropdownIcon,
	FolderOpenIcon,
	FolderSearchIcon,
	RefreshCwIcon,
	TrashIcon,
} from '@modrinth/assets'
import {
	ButtonStyled,
	Collapsible,
	Combobox,
	defineMessages,
	injectNotificationManager,
	Slider,
	StyledInput,
	useVIntl,
} from '@modrinth/ui'
import { open } from '@tauri-apps/plugin-dialog'
import { computed, ref, watch } from 'vue'

import ConfirmModalWrapper from '@/components/ui/modal/ConfirmModalWrapper.vue'
import { purge_cache_types } from '@/helpers/cache.js'
import { get, getDownloadSourceHealth, resetDownloadSourceHealth, set } from '@/helpers/settings.ts'
import { showAppDbBackupsFolder } from '@/helpers/utils.js'
import { useTheming } from '@/store/state'

const { handleError } = injectNotificationManager()
const themeStore = useTheming()
const settings = ref(await get())
const sourceHealth = ref(await getDownloadSourceHealth().catch(() => []))
const sourceHealthCollapsed = ref(true)
const purgeCacheConfirmModal = ref(null)
const { formatMessage } = useVIntl()

const messages = defineMessages({
	selectDirectory: {
		id: 'app.settings.resources.select-directory',
		defaultMessage: 'Select a new app directory',
	},
	appDirectory: { id: 'app.settings.resources.app-directory', defaultMessage: 'App directory' },
	appDirectoryDescription: {
		id: 'app.settings.resources.app-directory-description',
		defaultMessage:
			'The directory where the launcher stores all of its files. Changes apply after restarting the launcher.',
	},
	purgeConfirmTitle: {
		id: 'app.settings.resources.purge-confirm-title',
		defaultMessage: 'Are you sure you want to purge the cache?',
	},
	purgeConfirmDescription: {
		id: 'app.settings.resources.purge-confirm-description',
		defaultMessage:
			'If you proceed, your entire cache will be purged. This may slow down the app temporarily.',
	},
	appCache: { id: 'app.settings.resources.app-cache', defaultMessage: 'App cache' },
	purgeCache: { id: 'app.settings.resources.purge-cache', defaultMessage: 'Purge cache' },
	appCacheDescription: {
		id: 'app.settings.resources.app-cache-description',
		defaultMessage:
			'Axolotl Launcher caches data to speed up loading. Purging it forces the app to reload data and may temporarily slow the app down.',
	},
	downloadMirrors: {
		id: 'app.settings.resources.download-mirrors',
		defaultMessage: 'Download sources',
	},
	downloadMirrorsDescription: {
		id: 'app.settings.resources.download-mirrors-description',
		defaultMessage:
			'Automatic mode chooses between official and mirror sources based on your local environment and recent connection quality.',
	},
	automaticSource: {
		id: 'app.settings.resources.source.automatic',
		defaultMessage: 'Automatic (recommended)',
	},
	officialSource: {
		id: 'app.settings.resources.source.official',
		defaultMessage: 'Official only',
	},
	openBmclApiSource: {
		id: 'app.settings.resources.source.open-bmcl-api',
		defaultMessage: 'Prefer OpenBMCLAPI',
	},
	mcimSource: {
		id: 'app.settings.resources.source.mcim',
		defaultMessage: 'Prefer MCIM',
	},
	minecraftMetadataSource: {
		id: 'app.settings.resources.minecraft-metadata-source',
		defaultMessage: 'Minecraft metadata',
	},
	minecraftMetadataSourceDescription: {
		id: 'app.settings.resources.minecraft-metadata-source-description',
		defaultMessage: 'Version manifests and metadata for Minecraft and supported mod loaders.',
	},
	minecraftFileSource: {
		id: 'app.settings.resources.minecraft-file-source',
		defaultMessage: 'Minecraft files, loaders, and Java',
	},
	minecraftFileSourceDescription: {
		id: 'app.settings.resources.minecraft-file-source-description',
		defaultMessage: 'Game files, assets, libraries, mod loaders, and Java runtimes.',
	},
	modrinthMirror: {
		id: 'app.settings.resources.modrinth-mirror',
		defaultMessage: 'Modrinth',
	},
	modrinthMirrorDescription: {
		id: 'app.settings.resources.modrinth-mirror-description',
		defaultMessage: 'Modrinth public API requests and file downloads.',
	},
	curseforgeMirror: {
		id: 'app.settings.resources.curseforge-mirror',
		defaultMessage: 'CurseForge',
	},
	curseforgeMirrorDescription: {
		id: 'app.settings.resources.curseforge-mirror-description',
		defaultMessage: 'CurseForge public API requests and file downloads.',
	},
	sourceHealth: {
		id: 'app.settings.resources.source-health',
		defaultMessage: 'Source health',
	},
	sourceHealthDescription: {
		id: 'app.settings.resources.source-health-description',
		defaultMessage:
			'Connection quality is learned locally from completed downloads and is never uploaded.',
	},
	refreshSourceHealth: {
		id: 'app.settings.resources.source-health-refresh',
		defaultMessage: 'Refresh',
	},
	resetSourceHealth: {
		id: 'app.settings.resources.source-health-reset',
		defaultMessage: 'Reset learned data',
	},
	noSourceHealth: {
		id: 'app.settings.resources.source-health-empty',
		defaultMessage: 'No download measurements have been collected yet.',
	},
	sourceCoolingDown: {
		id: 'app.settings.resources.source-health-cooling-down',
		defaultMessage: 'Cooling down',
	},
	sourceRangeSplittingDisabled: {
		id: 'app.settings.resources.source-health-range-splitting-disabled',
		defaultMessage: 'Range splitting disabled',
	},
	sourceHealthStats: {
		id: 'app.settings.resources.source-health-stats',
		defaultMessage: '{ttfb} TTFB · {speed} · {successes} successful / {failures} failed',
	},
	maximumDownloads: {
		id: 'app.settings.resources.maximum-downloads',
		defaultMessage: 'Maximum concurrent downloads',
	},
	maximumDownloadsDescription: {
		id: 'app.settings.resources.maximum-downloads-description',
		defaultMessage:
			'Automatic mode selects between 16 and 64 concurrent downloads based on your device. Manual changes apply immediately.',
	},
	manualConcurrency: {
		id: 'app.settings.resources.concurrency.manual',
		defaultMessage: 'Manual',
	},
	maximumWrites: {
		id: 'app.settings.resources.maximum-writes',
		defaultMessage: 'Maximum concurrent writes',
	},
	maximumWritesDescription: {
		id: 'app.settings.resources.maximum-writes-description',
		defaultMessage:
			'The maximum number of files the launcher can write to disk at once. Use a lower value if you frequently get I/O errors. An app restart is required.',
	},
	databaseBackups: {
		id: 'app.settings.resources.database-backups',
		defaultMessage: 'App database backups',
	},
	openBackupsFolder: {
		id: 'app.settings.resources.open-backups-folder',
		defaultMessage: 'Open backups folder',
	},
	databaseBackupsDescription: {
		id: 'app.settings.resources.database-backups-description',
		defaultMessage:
			'Backups of important app data are stored here in case you need to recover them later.',
	},
})

function downloadSourceModel(setting) {
	return computed({
		get: () => settings.value[setting],
		set: (source) => {
			settings.value[setting] = source
		},
	})
}

const minecraftMetadataSource = downloadSourceModel('minecraft_metadata_source')
const minecraftFileSource = downloadSourceModel('minecraft_file_source')
const modrinthDownloadSource = downloadSourceModel('modrinth_source')
const curseforgeDownloadSource = downloadSourceModel('curseforge_source')
const automaticSourceOption = computed(() => ({
	value: 'auto',
	label: formatMessage(messages.automaticSource),
}))
const officialSourceOption = computed(() => ({
	value: 'official_only',
	label: formatMessage(messages.officialSource),
}))
const minecraftSourceOptions = computed(() => [
	automaticSourceOption.value,
	officialSourceOption.value,
	{ value: 'mirror_preferred', label: formatMessage(messages.openBmclApiSource) },
])
const mcimSourceOptions = computed(() => [
	automaticSourceOption.value,
	officialSourceOption.value,
	{ value: 'mirror_preferred', label: formatMessage(messages.mcimSource) },
])
const downloadConcurrencyMode = computed({
	get: () => (settings.value.auto_concurrent_downloads ? 'auto' : 'manual'),
	set: (mode) => {
		settings.value.auto_concurrent_downloads = mode === 'auto'
	},
})
const downloadConcurrencyOptions = computed(() => [
	{
		value: 'auto',
		label: formatMessage(messages.automaticSource),
	},
	{
		value: 'manual',
		label: formatMessage(messages.manualConcurrency),
	},
])

function sourceHealthSummary(source) {
	const ttfb = source.ttfb_ms == null ? '—' : `${Math.round(source.ttfb_ms)} ms`
	const speed =
		source.throughput_bytes_per_second == null
			? '—'
			: `${(source.throughput_bytes_per_second / 1024 / 1024).toFixed(1)} MiB/s`
	return formatMessage(messages.sourceHealthStats, {
		ttfb,
		speed,
		successes: source.successes,
		failures: source.failures,
	})
}

async function refreshSourceHealth() {
	sourceHealth.value = await getDownloadSourceHealth()
}

async function resetSourceHealth() {
	await resetDownloadSourceHealth()
	await refreshSourceHealth()
}

watch(
	settings,
	async () => {
		const setSettings = JSON.parse(JSON.stringify(settings.value))

		if (!setSettings.custom_dir) {
			setSettings.custom_dir = null
		}

		await set(setSettings)
	},
	{ deep: true },
)

async function purgeCache() {
	await purge_cache_types([
		'project',
		'project_v3',
		'version',
		'user',
		'team',
		'organization',
		'file',
		'loader_manifest',
		'minecraft_manifest',
		'categories',
		'report_types',
		'loaders',
		'game_versions',
		'donation_platforms',
		'file_hash',
		'file_update',
		'search_results',
		'search_results_v3',
	]).catch(handleError)
}

function handlePurgeCacheClick() {
	if (themeStore.getFeatureFlag('skip_non_essential_warnings')) {
		void purgeCache()
		return
	}

	purgeCacheConfirmModal.value?.show()
}

async function openDbBackupsFolder() {
	await showAppDbBackupsFolder().catch(handleError)
}

async function findLauncherDir() {
	const newDir = await open({
		multiple: false,
		directory: true,
		title: formatMessage(messages.selectDirectory),
	})

	if (newDir) {
		settings.value.custom_dir = newDir
	}
}
</script>

<template>
	<div class="flex flex-col gap-6">
		<div class="flex flex-col gap-2.5">
			<h2 class="m-0 text-lg font-semibold text-contrast">
				{{ formatMessage(messages.appDirectory) }}
			</h2>
			<StyledInput
				id="appDir"
				v-model="settings.custom_dir"
				:icon="BoxIcon"
				type="text"
				wrapper-class="w-full"
			>
				<template #right>
					<ButtonStyled circular>
						<button class="ml-1.5" @click="findLauncherDir">
							<FolderSearchIcon />
						</button>
					</ButtonStyled>
				</template>
			</StyledInput>
			<p class="m-0 leading-tight text-secondary">
				{{ formatMessage(messages.appDirectoryDescription) }}
			</p>
		</div>

		<div class="flex flex-col gap-2.5">
			<ConfirmModalWrapper
				ref="purgeCacheConfirmModal"
				:title="formatMessage(messages.purgeConfirmTitle)"
				:description="formatMessage(messages.purgeConfirmDescription)"
				:has-to-type="false"
				:proceed-label="formatMessage(messages.purgeCache)"
				:show-ad-on-close="false"
				@proceed="purgeCache"
			/>
			<h2 class="m-0 text-lg font-semibold text-contrast">
				{{ formatMessage(messages.appCache) }}
			</h2>
			<button id="purge-cache" class="btn min-w-max" @click="handlePurgeCacheClick">
				<TrashIcon />
				{{ formatMessage(messages.purgeCache) }}
			</button>
			<p class="m-0 leading-tight text-secondary">
				{{ formatMessage(messages.appCacheDescription) }}
			</p>
		</div>

		<div class="flex flex-col gap-3">
			<div>
				<h2 class="m-0 text-lg font-semibold text-contrast mt-4">
					{{ formatMessage(messages.downloadMirrors) }}
				</h2>
				<p class="m-0 leading-tight text-secondary">
					{{ formatMessage(messages.downloadMirrorsDescription) }}
				</p>
			</div>

			<div class="flex items-center justify-between gap-4">
				<div class="flex flex-col gap-1">
					<h3 class="m-0 text-base font-semibold text-contrast">
						{{ formatMessage(messages.minecraftMetadataSource) }}
					</h3>
					<p class="m-0 leading-tight text-secondary">
						{{ formatMessage(messages.minecraftMetadataSourceDescription) }}
					</p>
				</div>
				<div class="w-48 shrink-0">
					<Combobox v-model="minecraftMetadataSource" :options="minecraftSourceOptions" />
				</div>
			</div>

			<div class="flex items-center justify-between gap-4">
				<div class="flex flex-col gap-1">
					<h3 class="m-0 text-base font-semibold text-contrast">
						{{ formatMessage(messages.minecraftFileSource) }}
					</h3>
					<p class="m-0 leading-tight text-secondary">
						{{ formatMessage(messages.minecraftFileSourceDescription) }}
					</p>
				</div>
				<div class="w-48 shrink-0">
					<Combobox v-model="minecraftFileSource" :options="minecraftSourceOptions" />
				</div>
			</div>

			<div class="flex items-center justify-between gap-4">
				<div class="flex flex-col gap-1">
					<h3 class="m-0 text-base font-semibold text-contrast">
						{{ formatMessage(messages.modrinthMirror) }}
					</h3>
					<p class="m-0 leading-tight text-secondary">
						{{ formatMessage(messages.modrinthMirrorDescription) }}
					</p>
				</div>
				<div class="w-48 shrink-0">
					<Combobox v-model="modrinthDownloadSource" :options="mcimSourceOptions" />
				</div>
			</div>

			<div class="flex items-center justify-between gap-4">
				<div class="flex flex-col gap-1">
					<h3 class="m-0 text-base font-semibold text-contrast">
						{{ formatMessage(messages.curseforgeMirror) }}
					</h3>
					<p class="m-0 leading-tight text-secondary">
						{{ formatMessage(messages.curseforgeMirrorDescription) }}
					</p>
				</div>
				<div class="w-48 shrink-0">
					<Combobox v-model="curseforgeDownloadSource" :options="mcimSourceOptions" />
				</div>
			</div>
		</div>

		<div class="flex flex-col gap-2.5">
			<div class="flex items-start justify-between gap-4 mt-4">
				<button
					class="flex min-w-0 flex-1 items-start justify-between gap-2 border-0 bg-transparent p-0 text-left cursor-pointer"
					@click="sourceHealthCollapsed = !sourceHealthCollapsed"
				>
					<div>
						<h2 class="m-0 text-lg font-semibold text-contrast">
							{{ formatMessage(messages.sourceHealth) }}
						</h2>
						<p class="m-0 leading-tight text-secondary">
							{{ formatMessage(messages.sourceHealthDescription) }}
						</p>
					</div>
					<DropdownIcon
						class="mt-1 size-5 shrink-0 text-secondary transition-transform"
						:class="{ 'rotate-180': !sourceHealthCollapsed }"
					/>
				</button>
				<div class="flex shrink-0 gap-2">
					<ButtonStyled>
						<button @click="refreshSourceHealth().catch(handleError)">
							<RefreshCwIcon />
							{{ formatMessage(messages.refreshSourceHealth) }}
						</button>
					</ButtonStyled>
					<ButtonStyled>
						<button @click="resetSourceHealth().catch(handleError)">
							<TrashIcon />
							{{ formatMessage(messages.resetSourceHealth) }}
						</button>
					</ButtonStyled>
				</div>
			</div>
			<Collapsible :collapsed="sourceHealthCollapsed">
				<div class="flex flex-col gap-2 pt-3">
					<p v-if="sourceHealth.length === 0" class="m-0 text-secondary">
						{{ formatMessage(messages.noSourceHealth) }}
					</p>
					<div v-else class="flex flex-col gap-2">
						<div
							v-for="source in sourceHealth"
							:key="source.host"
							class="flex items-center justify-between gap-4 rounded-xl border border-divider bg-bg-raised px-4 py-3"
						>
							<div class="min-w-0">
								<div class="truncate font-semibold text-contrast">{{ source.host }}</div>
								<div class="text-sm text-secondary">{{ sourceHealthSummary(source) }}</div>
							</div>
							<div class="flex shrink-0 flex-col items-end gap-1 text-sm font-semibold">
								<span v-if="source.cooling_down" class="text-orange">
									{{ formatMessage(messages.sourceCoolingDown) }}
								</span>
								<span v-if="source.range_splitting_disabled" class="text-orange">
									{{ formatMessage(messages.sourceRangeSplittingDisabled) }}
								</span>
							</div>
						</div>
					</div>
				</div>
			</Collapsible>
		</div>

		<div class="flex flex-col gap-2.5">
			<div class="flex items-center justify-between gap-4 mt-4">
				<h2 class="m-0 text-lg font-semibold text-contrast">
					{{ formatMessage(messages.maximumDownloads) }}
				</h2>
				<div class="w-48 shrink-0">
					<Combobox v-model="downloadConcurrencyMode" :options="downloadConcurrencyOptions" />
				</div>
			</div>
			<Slider
				v-if="!settings.auto_concurrent_downloads"
				id="max-downloads"
				v-model="settings.max_concurrent_downloads"
				:min="1"
				:max="64"
				:step="1"
			/>
			<p class="m-0 leading-tight text-secondary">
				{{ formatMessage(messages.maximumDownloadsDescription) }}
			</p>
		</div>

		<div class="flex flex-col gap-2.5">
			<h2 class="mt-0 m-0 text-lg font-semibold text-contrast">
				{{ formatMessage(messages.maximumWrites) }}
			</h2>
			<Slider
				id="max-writes"
				v-model="settings.max_concurrent_writes"
				:min="1"
				:max="50"
				:step="1"
			/>
			<p class="m-0 leading-tight text-secondary">
				{{ formatMessage(messages.maximumWritesDescription) }}
			</p>
		</div>

		<div class="flex flex-col gap-2.5">
			<h2 class="mt-0 m-0 text-lg font-semibold text-contrast">
				{{ formatMessage(messages.databaseBackups) }}
			</h2>
			<button id="open-db-backups-folder" class="btn min-w-max" @click="openDbBackupsFolder">
				<FolderOpenIcon />
				{{ formatMessage(messages.openBackupsFolder) }}
			</button>
			<p class="m-0 leading-tight text-secondary">
				{{ formatMessage(messages.databaseBackupsDescription) }}
			</p>
		</div>
	</div>
</template>
