# Vendored Slint widgets

`fluent/` and `common/` are copies of the Slint 1.18.1 fluent style widgets
(`i-slint-compiler-1.18.1/widgets`), used as an ordinary widget library through
`../widgets.slint`. The compiler keeps the builtin `fluent-dark` style for
menus, context menus and tooltips. Upstream licence headers are retained.

HammeRR changes, all required because user code cannot use compiler internals:

- `fluent/styling.slint`: palette colours come from `../../theme.slint`; dark only.
- `fluent/color-scheme.slint`: fixed dark scheme instead of `SlintInternal`.
- `fluent/lineedit.slint`, `common/lineedit-base.slint`: explicit property
  forwarding instead of the experimental `interface`/`implement` feature.
- `common/combobox-base.slint`: per-row matching instead of experimental `index-of`.
- `fluent/scrollview.slint`, `textedit.slint`, `tableview.slint`: no `@deprecated`/`@shadowable`.
- Removed: menus, tab widget, radio group, tooltip, date and time pickers and
  `std-widgets.slint` (builtin-only elements or unused).
- Density: 26px control height and tighter padding in button, line edit,
  combo box, spin box and list items; 15px default button icons.

Re-copy and reapply these changes when upgrading Slint.
