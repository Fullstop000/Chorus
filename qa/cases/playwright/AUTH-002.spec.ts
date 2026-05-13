import { test, expect } from './helpers/fixtures'
import { gotoApp } from './helpers/ui'

/**
 * Catalog: `qa/cases/auth.md` — AUTH-002 Settings → Devices Mint + List + Rotate.
 *
 * Drives the end-to-end UI flow that the PRD's onboarding loop depends
 * on:
 *
 * 1. Open Settings → Devices on a fresh worker (no bridge token yet).
 * 2. Click "Onboard a device" → the reveal panel appears with the
 *    one-time script and a Copy button. Script contains a `chrs_bridge_`
 *    bearer literal.
 * 3. Dismiss the reveal panel. The token row now exists; the page
 *    swaps the CTA from "Onboard a device" to "Rotate token".
 * 4. Click Rotate → confirm. A NEW reveal panel appears with a script
 *    whose bearer differs from the first.
 *
 * Coverage focus: the HTTP surface (`/api/devices/mint`, /rotate) AND
 * the UI's "shown once" property. Kick / Forget / state-machine edges
 * are exercised by the Rust integration tests
 * (`tests/devices_tests.rs`).
 */
test.describe('AUTH-002', () => {
  test('Settings → Devices Mint + List + Rotate @case AUTH-002', async ({ page }) => {
    await gotoApp(page)

    // Open the Settings overlay via the gear icon in the sidebar footer.
    await page.locator('button[aria-label="Open settings"]').click()
    await expect(page.locator('.settings-page')).toBeVisible()

    // Switch to the Devices section.
    await page.locator('.settings-nav-item:text("Devices")').click()
    await expect(page.locator('.settings-section-title:text("Devices")')).toBeVisible()

    // Empty state: no rows yet, CTA visible.
    await expect(page.locator('.devices-empty')).toBeVisible()
    const mintCta = page.locator('button:has-text("Onboard a device")')
    await expect(mintCta).toBeVisible()

    // Mint. The reveal panel appears with the script + Copy button.
    const mintResponse = page.waitForResponse(
      (res) => res.url().endsWith('/api/devices/mint') && res.status() === 200,
    )
    await mintCta.click()
    await mintResponse
    const reveal = page.locator('.devices-reveal')
    await expect(reveal).toBeVisible()
    const script = await reveal.locator('.devices-reveal-script').textContent()
    expect(script ?? '', 'rendered script must include bridge token literal').toMatch(
      /chrs_bridge_/,
    )
    expect(script ?? '').toContain('exec chorus bridge')

    // Save the first token so we can verify rotation produces a different one.
    const firstScript = script ?? ''

    // Dismiss → returns to list view, but now with a row visible? No —
    // mint-only-route doesn't actually register a bridge machine; only
    // an actual `bridge.hello` does. The list stays empty until a real
    // bridge connects. So we just verify the CTA flipped.
    await reveal.locator('button:has-text("I\'ve saved it")').click()
    await expect(reveal).toBeHidden()
    await expect(mintCta).toBeHidden()
    const rotateCta = page.locator('button:has-text("Rotate token")')
    await expect(rotateCta).toBeVisible()

    // Rotate → confirm → second reveal panel.
    await rotateCta.click()
    const confirmRotate = page.locator('button:has-text("Yes, rotate")')
    await expect(confirmRotate).toBeVisible()
    const rotateResponse = page.waitForResponse(
      (res) => res.url().endsWith('/api/devices/rotate') && res.status() === 200,
    )
    await confirmRotate.click()
    await rotateResponse

    await expect(reveal).toBeVisible()
    const secondScript = (await reveal.locator('.devices-reveal-script').textContent()) ?? ''
    expect(secondScript).toMatch(/chrs_bridge_/)
    expect(secondScript).not.toEqual(firstScript)
  })
})
