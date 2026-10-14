import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import tailwindcss from '@tailwindcss/vite'
import vue from '@vitejs/plugin-vue'
import { defineConfig } from 'vite'

const __dirname = path.dirname(fileURLToPath(import.meta.url))
const DEFAULT_ADMIN_ADDR = '127.0.0.1:8001'

function resolveAdminAddress() {
  // Highest priority: explicit override.
  if (process.env.SAMA_API_TARGET) return process.env.SAMA_API_TARGET

  // Otherwise read admin_listen_address (or legacy management_listen_address)
  // from the sama config.toml so `npm run dev` follows config.toml.
  // Config path override: SAMA_CONFIG=../custom.toml npm run dev
  const configPath =
    process.env.SAMA_CONFIG ??
    path.resolve(__dirname, '../config.toml')
  try {
    const text = fs.readFileSync(configPath, 'utf-8')
    const m =
      text.match(/^\s*admin_listen_address\s*=\s*["']([^"']+)["']/m) ??
      text.match(/^\s*management_listen_address\s*=\s*["']([^"']+)["']/m)
    if (m) return `http://${m[1]}`
  } catch {
    // Missing/unreadable config: fall through to default.
  }
  return `http://${DEFAULT_ADMIN_ADDR}`
}

// https://vite.dev/config/
export default defineConfig({
  plugins: [vue(), tailwindcss()],
  server: {
    proxy: {
      // Dev: Vite on :5173 forwards /api to the Rust admin server.
      // Follows config.toml's admin_listen_address automatically.
      // Overrides: SAMA_API_TARGET=http://127.0.0.1:9999 npm run dev
      //            SAMA_CONFIG=../custom.toml npm run dev
      '/api': {
        target: resolveAdminAddress(),
        changeOrigin: true,
      },
    },
  },
})
