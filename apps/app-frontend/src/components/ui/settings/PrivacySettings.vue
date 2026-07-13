<script setup lang="ts">
import { defineMessages, Toggle, useVIntl } from '@modrinth/ui'
import { ref, watch } from 'vue'

import { optInAnalytics, optOutAnalytics } from '@/helpers/analytics'
import { get, set } from '@/helpers/settings.ts'

const settings = ref(await get())
const { formatMessage } = useVIntl()
const messages = defineMessages({
	diagnosticsTitle: {
		id: 'app.settings.privacy.diagnostics-title',
		defaultMessage: 'Optional diagnostics',
	},
	diagnosticsDescription: {
		id: 'app.settings.privacy.diagnostics-description',
		defaultMessage:
			'{productName} does not send interface analytics in this build. This opt-in is retained for a future {organizationShortName}-operated diagnostics service and is disabled by default.',
	},
	discordTitle: {
		id: 'app.settings.privacy.discord-title',
		defaultMessage: 'Discord RPC',
	},
	discordDescription: {
		id: 'app.settings.privacy.discord-description',
		defaultMessage:
			'Manages the Discord Rich Presence integration. Disabling this will prevent {productShortName} from appearing as a game or app on your Discord profile.',
	},
	discordNote: {
		id: 'app.settings.privacy.discord-note',
		defaultMessage:
			'Instance-specific Discord Rich Presence integrations added by mods are not affected. Restart the app to apply this setting.',
	},
})

watch(
	settings,
	async () => {
		if (settings.value.telemetry) {
			optInAnalytics()
		} else {
			optOutAnalytics()
		}

		await set(settings.value)
	},
	{ deep: true },
)
</script>

<template>
	<div class="flex items-center justify-between gap-4">
		<div>
			<h2 class="m-0 text-lg font-semibold text-contrast">
				{{ formatMessage(messages.diagnosticsTitle) }}
			</h2>
			<p class="m-0 mt-1 text-sm">
				{{
					formatMessage(messages.diagnosticsDescription, {
						productName: AxolotlBrandConfig.productName,
						organizationShortName: AxolotlBrandConfig.shortOrganizationName,
					})
				}}
			</p>
		</div>
		<Toggle id="opt-out-analytics" v-model="settings.telemetry" />
	</div>

	<div class="mt-4 flex items-center justify-between gap-4">
		<div>
			<h2 class="m-0 text-lg font-semibold text-contrast">
				{{ formatMessage(messages.discordTitle) }}
			</h2>
			<p class="m-0 mt-1 text-sm">
				{{
					formatMessage(messages.discordDescription, {
						productShortName: AxolotlBrandConfig.shortProductName,
					})
				}}
			</p>
			<p class="m-0 mt-2 text-sm">
				{{ formatMessage(messages.discordNote) }}
			</p>
		</div>
		<Toggle id="disable-discord-rpc" v-model="settings.discord_rpc" />
	</div>
</template>
import { AxolotlBrandConfig } from '@/config'
