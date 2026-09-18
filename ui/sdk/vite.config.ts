import { defineConfig } from 'vite'
import dts from 'vite-plugin-dts'

export default defineConfig({
  plugins: [dts({ rollupTypes: true })],
  build: {
    lib: {
      entry: 'src/index.ts',
      name: 'MeghAuth',
      formats: ['es', 'cjs'],
      fileName: (format) => `megh-auth.${format}.js`,
    },
  },
})
