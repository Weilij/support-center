import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    environment: 'jsdom',
    fileParallelism: false,
    maxWorkers: 1,
    pool: 'threads',
  },
})
