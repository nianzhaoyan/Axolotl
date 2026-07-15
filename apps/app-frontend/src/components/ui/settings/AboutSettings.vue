<script setup lang="ts">
import { ExternalIcon } from '@modrinth/assets'
import { defineMessages, useVIntl } from '@modrinth/ui'
import { getVersion } from '@tauri-apps/api/app'

import { AxolotlBrandConfig } from '@/config'

const { formatMessage } = useVIntl()
const version = await getVersion()

const messages = defineMessages({
	productTitle: {
		id: 'app.settings.about.product-title',
		defaultMessage: 'About {productName}',
	},
	version: {
		id: 'app.settings.about.version',
		defaultMessage: 'Version {version}',
	},
	developer: {
		id: 'app.settings.about.developer',
		defaultMessage: 'Developed by {developerName} at {organizationName}.',
	},
	attribution: {
		id: 'app.settings.about.attribution',
		defaultMessage: 'This application is a modified version of the open-source Modrinth project.',
	},
	originalSource: {
		id: 'app.settings.about.original-source',
		defaultMessage: 'View the original Modrinth source code',
	},
	projectWebsite: {
		id: 'app.settings.about.project-website',
		defaultMessage: 'Visit the project website',
	},
})
</script>

<template>
	<div class="flex flex-col gap-6">
		<div class="flex items-center gap-4">
			<img class="size-20 object-contain" src="@/assets/axolotl.png" alt="" />
			<div>
				<h2 class="m-0 text-xl font-semibold text-contrast">
					{{
						formatMessage(messages.productTitle, {
							productName: AxolotlBrandConfig.productName,
						})
					}}
				</h2>
				<p class="m-0 mt-1 text-secondary">
					{{ formatMessage(messages.version, { version }) }}
				</p>
			</div>
		</div>

		<div class="rounded-xl bg-surface-4 p-4">
			<p class="m-0 text-primary">
				{{
					formatMessage(messages.developer, {
						developerName: AxolotlBrandConfig.developerName,
						organizationName: AxolotlBrandConfig.organizationName,
					})
				}}
			</p>
			<p class="m-0 mt-3 text-primary">
				{{ formatMessage(messages.attribution) }}
			</p>
		</div>

		<div class="flex flex-col items-start gap-3">
			<a
				href="https://github.com/modrinth/code"
				target="_blank"
				rel="noopener noreferrer"
				class="inline-flex items-center gap-2 font-semibold text-brand hover:underline"
			>
				{{ formatMessage(messages.originalSource) }}
				<ExternalIcon class="size-4" />
			</a>
			<a
				:href="AxolotlBrandConfig.website"
				target="_blank"
				rel="noopener noreferrer"
				class="inline-flex items-center gap-2 font-semibold text-brand hover:underline"
			>
				{{ formatMessage(messages.projectWebsite) }}
				<ExternalIcon class="size-4" />
			</a>
		</div>
	</div>
</template>
