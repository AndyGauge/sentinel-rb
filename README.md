# 🛡️ Sentinel

**Sentinel** is a high-performance Rust-powered watcher that keeps your Ruby code and RBS type signatures in perfect sync. It bridges the gap between dynamic Ruby models and static RBS type definitions.

## 🚀 Getting Started

### 1. Install the Gem
Add Sentinel to your Gemfile:
```
    group :development do
      gem 'sentinel'
    end
```
Then run:
```
    bundle install
```
### 2. Initialize the Project
Run the following command to set up the necessary directories and generate RBS files:
```
    bundle exec sentinel init
```
This creates a `.sentinel.toml` config file (if one doesn't exist) with `app` as the default watched folder, then generates RBS signatures for all Ruby files in it.

### 3. Configure Watched Folders

Sentinel watches the `app` folder by default. Use the CLI to add or remove folders:

```bash
# Add a folder
bundle exec sentinel add lib

# Add another
bundle exec sentinel add config/initializers

# Remove a folder
bundle exec sentinel remove app

# List current configuration
bundle exec sentinel list
```

The configuration is stored in `.sentinel.toml` at your project root:
```toml
folders = ["app", "lib"]
output = "sig/generated"
```

You can also edit this file directly. Sentinel reads it on every `init` and `watch` command.

---

## 🔍 Checking Signatures in CI / Pre-commit

Use `sentinel check` to verify that generated RBS files are up to date **without modifying anything**. It exits with code 1 if any signatures are missing or stale.

```bash
bundle exec sentinel check
```

### GitHub Actions

```yaml
- name: Check RBS signatures are up to date
  run: bundle exec sentinel check
```

### Git Pre-commit Hook

Add the following to `.git/hooks/pre-commit` (or use a framework like [Lefthook](https://github.com/evilmartians/lefthook) or [Husky](https://github.com/typicode/husky)):

```bash
#!/usr/bin/env bash
set -e

bundle exec sentinel check
```

Then make it executable:
```bash
chmod +x .git/hooks/pre-commit
```

If the check fails, run `bundle exec sentinel init` to regenerate, review the changes, and commit again.

---

## 🛠️ Editor Setup

To get live type-checking diagnostics, your editor needs to talk to Steep (the Ruby Type Server), which monitors the signatures Sentinel generates.

### Neovim Setup
Add this to your LSP configuration (e.g., lsp.lua). This ensures Steep is aware of the file changes Sentinel makes in the background.
```
    require('lspconfig').steep.setup({
      cmd = { "bundle", "exec", "steep", "langserver" },
      capabilities = {
        workspace = {
          didChangeWatchedFiles = { dynamicRegistration = true },
        },
      },
      settings = {
        steep = {
          check_on_save = true,
          enable_diagnostics = true,
        }
      }
    })
```
### VS Code Setup
1. Install the Steep VS Code extension.
2. Open your settings.json and ensure it points to your bundled Steep:
```
    {
      "steep.command": "bundle exec steep langserver",
      "steep.enableDiagnostics": true
    }
```
3. Sentinel runs as a separate process; VS Code will automatically pick up the generated .rbs files.

---

## 🔄 How it Works

Sentinel is designed to be a "set-and-forget" background service:

1. The Watcher: You run 'bundle exec sentinel watch'. It stays active, monitoring your configured folders (default: app).
2. The Transpiler: The moment you save a Ruby file, Sentinel's Rust engine generates a corresponding .rbs file in sig/generated.
3. The Feedback: Your editor (via Steep) sees the new .rbs and immediately updates the diagnostics in your Ruby file.

---
```
-- Add this to your lsp.lua, outside the return table or inside on_attach
local function start_sentinel()
  if _G.sentinel_job_id then return end -- Don't start it twice

  _G.sentinel_job_id = vim.fn.jobstart({ "bundle", "exec", "sentinel", "watch" }, {
    detach = true, -- Process keeps running in background
    on_stderr = function(_, data)
      -- Optional: print errors to Neovim's :messages if something breaks
      if data and data[1] ~= "" then
        vim.schedule(function()
          vim.notify("Sentinel Error: " .. table.concat(data, "\n"), vim.log.levels.ERROR)
        end)
      end
    end,
    on_exit = function()
      _G.sentinel_job_id = nil
    end,
  })
end

-- Update your steep setup to trigger this
require('lspconfig').steep.setup({
  on_attach = function(client, bufnr)
    start_sentinel()
    -- ... rest of your on_attach
  end,
  -- ... rest of your config
})---
```
## ⚠️ Troubleshooting

If Steep fails to start with "Exit Code 2", ensure you are running within the correct environment. Sentinel depends on the bundle context to resolve paths correctly. If using asdf or rbenv, ensure your shims are updated:

    asdf reshim ruby
