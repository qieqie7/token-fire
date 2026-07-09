# TokenFire Compact Widget Design Direction

## Product Mood

TokenFire is a lightweight macOS instrument for AI token usage. The primary surface is a compact floating widget.

The UI should feel:

- precise
- real-time
- low-interruption
- energetic without visual noise
- developer-tool native
- closer to an instrument panel than a SaaS dashboard

Numbers are the main product surface. Every layout decision should protect numeric stability and readability.

## Styling System

Use:

- React + TypeScript
- CSS variables for tokens
- plain CSS for layout, component states, and animation

Do not use in this iteration:

- Tailwind
- styled-components
- decorative SVG backgrounds
- image assets
- large blue or purple gradients
- gradient orbs

Reasoning:

- TokenFire needs a small, inspectable widget style system, not a large design-system canvas.
- CSS variables make token-to-style mapping inspectable.
- Plain CSS keeps Tauri widget runtime small and predictable.
- Odometer animation and status states are easier to audit with semantic classes and `data-*` attributes.

## Token Naming

Use dot.case for design tokens:

- `color.brand.primary`
- `color.brand.primary.100`
- `color.surface.base`
- `color.text.primary`
- `space.4`
- `font.size.md`

Use kebab-case for CSS variables:

- `--color-brand-primary`
- `--color-brand-primary-100`
- `--color-surface-base`
- `--color-text-primary`
- `--space-4`
- `--font-size-md`

## Spacing, Radius, And Shadow

Spacing uses a 4px base scale:

- `space.0 = 0`
- `space.1 = 4px`
- `space.2 = 8px`
- `space.3 = 12px`
- `space.4 = 16px`
- `space.5 = 20px`
- `space.6 = 24px`
- `space.8 = 32px`
- `space.10 = 40px`
- `space.12 = 48px`

Semantic aliases:

- `xs = space.1`
- `sm = space.2`
- `md = space.4`
- `lg = space.6`
- `xl = space.8`
- `2xl = space.12`

Radius should stay restrained. Prefer 6px or 8px for component frames. Avoid large rounded cards unless the component needs a pill shape.

Shadow should be subtle and functional. Use it for raised overlays, focus separation, and floating surfaces. Do not use shadow as decoration.

## Color Principles

Dark mode is primary.

Use green-cyan energy for live growth, but avoid a one-note neon theme. Neutral surfaces carry most of the UI. Status colors are reserved for state.

The compact widget only needs the token set it uses:

- widget surface
- primary text
- secondary text
- muted text
- border
- status green
- status yellow
- status red

Do not add palette galleries or broad 100-900 scales unless a future full design-system task explicitly needs them.

## Typography Principles

Metric typography is the hero.

Use:

- `font-variant-numeric: tabular-nums`
- mono or semi-mono numeric treatment
- stable width containers
- zero letter spacing unless a specific text style declares otherwise

Do not let digit changes resize the component.

## Odometer Motion

Live token numbers should animate like a restrained odometer:

- digits move vertically
- each digit owns a fixed-width slot
- transforms drive motion
- no layout shift
- running state advances in small increments
- completion decelerates into the final value
- error state stops motion and uses status color
- reduced motion disables rolling digits

Use subtle scanline or pulse feedback only during `running`. It must never compete with the number.

## Drag Region Rules

The main Tauri window must be movable.

The whole compact widget shell is draggable by click-and-hold.

Do not add a visible drag bar. Do not show `Drag window` text. Do not spend vertical space on window chrome.

Keep the window compact and floating:

- `decorations: false`
- `transparent: true`
- `alwaysOnTop: true`
- `resizable: false`

Future interactive controls must opt out of dragging with `data-no-drag`.

Never put these inside a drag region:

- button
- input
- select
- checkbox
- radio
- toggle
- tooltip trigger
- any future control that needs pointer interaction

## Component Boundaries

Keep these boundaries:

- `main.tsx`: React mount only.
- `App.tsx`: compact widget shell, whole-window drag, and app-level wiring.
- `useWidgetState.ts`: React state hook for the existing `widget_state` command.
- `widgetStatePolling.ts`: polling behavior that preserves immediate load, 1s interval, fallback, and stale-response protection.
- `tokens.css`: CSS custom properties.
- `widget/types.ts`: shared `WidgetState` and `WidgetStatus`.
- `widget/format.ts`: token formatting helpers.
- `widget/odometer.ts`: stable digit slot helpers.
- `widget/LiveTokenCounter.tsx`: compact metric view.
- `widget/StatusPill.tsx`: status label treatment.

Do not change Rust token accounting, Traex parsing, SQLite storage, or hook intake for visual-system work unless the user explicitly asks.

Do not create a large `DesignSystemCanvas.tsx`.

Do not create a 12-component gallery.

## Required Widget Pieces

The compact widget must render:

- today's total token count from `widget_state.today_total_tokens`
- latest turn delta from `widget_state.latest_turn_delta_tokens`
- status label from `widget_state.status_label`
- visual status from `widget_state.status`
- stable numeric slots for the main token count
- reduced-motion behavior for digit transitions

The React implementation must preserve the old frontend data behavior:

- call `invoke<WidgetState>("widget_state")`
- load immediately
- refresh every 1s
- render fallback state on load failure
- ignore stale responses that resolve after newer responses

## Naming Conventions

Use:

- React components: PascalCase
- Props: camelCase
- Tokens: dot.case
- CSS variables: `--color-status-green`
- Test ids: kebab-case

## Verification Expectations

Before marking a compact widget implementation complete:

- run `corepack pnpm build`
- run `corepack pnpm test`
- inspect the widget near `240 x 160`
- confirm the window remains small and floating
- verify no text overflow
- verify no token-number layout shift
- verify click-and-hold anywhere on the widget shell drags the Tauri window
- verify there is no visible drag bar or `Drag window` text
- verify reduced motion
