<script setup lang="ts">
import { Accordion, defineMessages, useVIntl } from '@modrinth/ui'
import { computed } from 'vue'

import {
	getAnnouncementByVersion,
	getAnnouncements,
	getLocalizedAnnouncementText,
} from '@/announcements/catalog'
import { AxolotlBrandConfig } from '@/config'
import i18n from '@/i18n.config'

import UpdateAnnouncementContent from './UpdateAnnouncementContent.vue'

const props = defineProps<{
	currentVersion: string
}>()

const { formatMessage } = useVIntl()

const messages = defineMessages({
	title: {
		id: 'app.settings.updates.announcements.title',
		defaultMessage: 'Update announcements',
	},
	description: {
		id: 'app.settings.updates.announcements.description',
		defaultMessage: 'See what changed in this version and browse previous releases.',
	},
	history: {
		id: 'app.settings.updates.announcements.history',
		defaultMessage: 'Version history',
	},
	empty: {
		id: 'app.settings.updates.announcements.empty',
		defaultMessage: 'No bundled update announcements are available.',
	},
})

const locale = computed(() => i18n.global.locale.value)
const launcherAnnouncements = getAnnouncements()
const currentAnnouncement = computed(() => getAnnouncementByVersion(props.currentVersion))
const historyAnnouncements = computed(() =>
	launcherAnnouncements.filter((announcement) => announcement.id !== currentAnnouncement.value?.id),
)
</script>

<template>
	<section class="flex min-w-0 flex-col gap-6 border-0 border-t border-solid border-surface-5 pt-6">
		<div class="flex min-w-0 flex-col gap-1">
			<h2 class="m-0 text-lg font-semibold text-contrast">
				{{ formatMessage(messages.title) }}
			</h2>
			<p class="m-0 leading-relaxed text-secondary">
				{{ formatMessage(messages.description) }}
			</p>
		</div>

		<div class="border-0 border-b border-solid border-surface-5 pb-6">
			<UpdateAnnouncementContent
				:announcement="currentAnnouncement"
				:version="currentVersion"
				:external-url="currentAnnouncement?.externalUrl ?? AxolotlBrandConfig.website"
			/>
		</div>

		<div class="flex min-w-0 flex-col gap-3">
			<h3 class="m-0 text-base font-semibold text-contrast">
				{{ formatMessage(messages.history) }}
			</h3>
			<p v-if="historyAnnouncements.length === 0" class="m-0 text-sm text-secondary">
				{{ formatMessage(messages.empty) }}
			</p>
			<div
				v-else
				class="divide-y divide-solid divide-surface-5 border-y border-solid border-surface-5"
			>
				<Accordion
					v-for="announcement in historyAnnouncements"
					:key="announcement.id"
					class="min-w-0"
					button-class="group flex w-full cursor-pointer items-center gap-3 border-0 bg-transparent px-0 py-4 text-left"
					content-class="pb-5"
				>
					<template #title>
						<span class="flex min-w-0 flex-1 flex-wrap items-baseline gap-x-3 gap-y-1">
							<span class="truncate font-semibold text-primary group-hover:text-contrast">
								{{ getLocalizedAnnouncementText(announcement.title, locale) }}
							</span>
							<time class="text-sm font-normal text-secondary" :datetime="announcement.publishedAt">
								{{ announcement.publishedAt }}
							</time>
						</span>
					</template>
					<div class="border-0 border-t border-solid border-surface-5 pt-5">
						<UpdateAnnouncementContent
							:announcement="announcement"
							:show-header="false"
							:external-url="announcement.externalUrl"
						/>
					</div>
				</Accordion>
			</div>
		</div>
	</section>
</template>
