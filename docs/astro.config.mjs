import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import md3Theme from 'starlight-theme-md3';

export default defineConfig({
  site: process.env.SITE_URL || 'https://getdoorman.dev',
  integrations: [starlight({
    title: 'doorman',
    description: 'Friendly HTTPS names for local apps. Learn, integrate, and develop with Doorman.',
    favicon: '/favicon.svg',
    logo: { src: './public/favicon.svg', alt: '' },
    plugins: [md3Theme({ seed: '#345b94', variant: 'tonalSpot', density: 'comfortable', shape: 'medium', motion: false })],
    customCss: ['./src/styles/custom.css'],
    social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/brunokiafuka/doorman' }],
    editLink: { baseUrl: 'https://github.com/brunokiafuka/doorman/edit/main/docs/' },
    sidebar: [
      { label: 'Start here', items: [
        { label: 'What is Doorman?', slug: 'start/overview' },
        { label: 'Install on macOS', slug: 'start/install' },
        { label: 'Run your first app', slug: 'start/first-app' },
      ] },
      { label: 'Bring your team', items: [
        { label: 'Add to a project', slug: 'teams/add-to-project' },
        { label: 'Monorepos & services', slug: 'teams/monorepos' },
        { label: 'Onboard teammates', slug: 'teams/onboarding' },
      ] },
      { label: 'Everyday workflows', items: [
        { label: 'Branches & worktrees', slug: 'guides/worktrees' },
        { label: 'HTTPS & custom domains', slug: 'guides/domains' },
        { label: 'Inspect & share', slug: 'guides/inspect-and-share' },
        { label: 'Troubleshooting', slug: 'guides/troubleshooting' },
      ] },
      { label: 'Reference', items: [
        { label: 'Project configuration', slug: 'reference/configuration' },
        { label: 'CLI commands', slug: 'reference/cli' },
        { label: 'Security & architecture', slug: 'reference/architecture' },
        { label: 'Build from source', slug: 'reference/development' },
      ] },
    ],
  })],
});
