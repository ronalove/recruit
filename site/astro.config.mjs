// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import starlightLinksValidator from 'starlight-links-validator';

export default defineConfig({
  site: 'https://ronalove.github.io',
  base: '/recruit',
  integrations: [
    starlight({
      title: 'recruit',
      disable404Route: true,
      social: [
        { icon: 'github', label: 'GitHub', href: 'https://github.com/ronalove/recruit' },
      ],
      defaultLocale: 'root',
      locales: {
        root: { label: 'English', lang: 'en' },
        fr: { label: 'Français', lang: 'fr' },
      },
      sidebar: [
        {
          label: 'Getting started',
          translations: { fr: 'Prise en main' },
          items: [{ autogenerate: { directory: 'getting-started' } }],
        },
        {
          label: 'Guides',
          items: [{ autogenerate: { directory: 'guides' } }],
        },
        {
          label: 'Reference',
          translations: { fr: 'Référence' },
          items: [{ autogenerate: { directory: 'reference' } }],
        },
        { slug: 'faq' },
      ],
      plugins: [starlightLinksValidator()],
    }),
  ],
});
