# Lantern

Fast Linux photo browser (Rust, GTK 4, libadwaita). Read `docs/PLAN.md` before
making changes; it is the source of truth for scope and architecture.

- Never add a persistent thumbnail cache, index, or database.
- Never hard-delete files; delete means move to trash.
- Keep the feature list as small as the plan says. Ask before adding features.
- Performance claims need a measurement on a real folder, not a guess.
