import { defineConfig } from 'astro/config';
import tailwindcss from '@tailwindcss/vite';

export default defineConfig({
  site: 'https://crescent617.github.io',
  base: '/yomi',
  vite: {
    plugins: [tailwindcss()],
  },
});
