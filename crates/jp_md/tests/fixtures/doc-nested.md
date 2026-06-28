# Configuration Guide

The settings below are applied in order, with later layers overriding earlier
ones. Most projects only need the first two.

1. **Defaults.** The built-in values, used when nothing else is set.
2. **Workspace config.** Read from the workspace root:

   ```toml
   [model]
   provider = "anthropic"
   alias = "sonnet"
   ```

3. **Environment.** Any `JP_CFG_*` variable wins over the file.

A few notes on precedence and nesting:

- Lists nest by indentation.
  - Two spaces per level is the convention used throughout this guide.
  - Deeper items continue to render at their own visual column.
- A blank line between items makes the surrounding list loose.

That is the whole model; everything else is a special case of it.
