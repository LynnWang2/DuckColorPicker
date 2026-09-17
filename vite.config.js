import { defineConfig } from 'vite';
import { resolve } from 'node:path';

export default defineConfig({
  clearScreen: false,
  server: { strictPort: true },
  build: {
    target: ['es2021', 'safari13'],
    rollupOptions: {
      input: {
        main: resolve(import.meta.dirname, 'index.html'),
        picker: resolve(import.meta.dirname, 'picker.html')
      }
    }
  }
});
