import { test } from '@playwright/test'

export function definePlaceholderCase(options: {
  id: string
  title: string
  catalogPath: string
  reason: string
}): void {
  test.describe(options.id, () => {
    test(`${options.title} @case ${options.id}`, async () => {
      test.fixme(`${options.catalogPath} — ${options.reason}`)
    })
  })
}
