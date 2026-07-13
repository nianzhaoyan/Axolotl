<script setup>
import { BoxIcon, FolderOpenIcon, FolderSearchIcon, TrashIcon } from '@modrinth/assets'
import {
	ButtonStyled,
	defineMessages,
	injectNotificationManager,
	Slider,
	StyledInput,
	useVIntl,
} from '@modrinth/ui'
import { open } from '@tauri-apps/plugin-dialog'
import { ref, watch } from 'vue'

import ConfirmModalWrapper from '@/components/ui/modal/ConfirmModalWrapper.vue'
import { purge_cache_types } from '@/helpers/cache.js'
import { get, set } from '@/helpers/settings.ts'
import { showAppDbBackupsFolder } from '@/helpers/utils.js'
import { useTheming } from '@/store/state'

const { handleError } = injectNotificationManager()
const themeStore = useTheming()
const settings = ref(await get())
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
	maximumDownloads: {
		id: 'app.settings.resources.maximum-downloads',
		defaultMessage: 'Maximum concurrent downloads',
	},
	maximumDownloadsDescription: {
		id: 'app.settings.resources.maximum-downloads-description',
		defaultMessage:
			'The maximum number of files the launcher can download at the same time. Use a lower value on a poor internet connection. An app restart is required.',
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

		<div class="flex flex-col gap-2.5">
			<h2 class="m-0 text-lg font-semibold text-contrast mt-4">
				{{ formatMessage(messages.maximumDownloads) }}
			</h2>
			<Slider
				id="max-downloads"
				v-model="settings.max_concurrent_downloads"
				:min="1"
				:max="10"
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
