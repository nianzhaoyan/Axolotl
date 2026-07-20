import { AbstractFeature, type RequestContext } from '@modrinth/api-client'

import { getOfficialLabrinthBaseUrl, MODRINTH_MIRROR_BASE_URL } from '@/config'

function withoutSensitiveHeaders(headers: Record<string, string> | undefined) {
	return Object.fromEntries(
		Object.entries(headers ?? {}).filter(([name]) => {
			const normalizedName = name.toLowerCase()
			return normalizedName !== 'authorization' && normalizedName !== 'modrinth-download-meta'
		}),
	)
}

export class ModrinthMirrorFallbackFeature extends AbstractFeature {
	shouldApply(context: RequestContext) {
		return (
			super.shouldApply(context) &&
			(context.options.method ?? 'GET') === 'GET' &&
			context.options.api === 'labrinth' &&
			context.url.startsWith(MODRINTH_MIRROR_BASE_URL)
		)
	}

	async execute<T>(next: () => Promise<T>, context: RequestContext): Promise<T> {
		const mirrorUrl = context.url
		const originalHeaders = context.options.headers
		context.options.headers = withoutSensitiveHeaders(originalHeaders)
		const mirrorStarted = performance.now()
		console.info('[modrinth-mirror] Attempting Modrinth API request', {
			source: 'Mirror',
			method: context.options.method ?? 'GET',
			url: mirrorUrl,
			route: 1,
		})

		try {
			const result = await next()
			console.info('[modrinth-mirror] Completed Modrinth API request', {
				source: 'Mirror',
				url: mirrorUrl,
				elapsedMs: Math.round(performance.now() - mirrorStarted),
			})
			return result
		} catch (error) {
			context.url = `${getOfficialLabrinthBaseUrl()}${mirrorUrl.slice(MODRINTH_MIRROR_BASE_URL.length)}`
			context.options.headers = originalHeaders
			console.warn('[modrinth-mirror] Mirror request failed; falling back to official source', {
				url: mirrorUrl,
				elapsedMs: Math.round(performance.now() - mirrorStarted),
				error,
			})
			const officialStarted = performance.now()
			console.info('[modrinth-mirror] Attempting Modrinth API request', {
				source: 'Official',
				method: context.options.method ?? 'GET',
				url: context.url,
				route: 2,
			})
			try {
				const result = await next()
				console.info('[modrinth-mirror] Completed Modrinth API request', {
					source: 'Official',
					url: context.url,
					elapsedMs: Math.round(performance.now() - officialStarted),
				})
				return result
			} catch (officialError) {
				console.error('[modrinth-mirror] Official Modrinth API request failed', {
					url: context.url,
					elapsedMs: Math.round(performance.now() - officialStarted),
					error: officialError,
				})
				throw officialError
			}
		} finally {
			context.url = mirrorUrl
			context.options.headers = originalHeaders
		}
	}
}
