export interface NavItem {
  text: string;
  link: string;
}

export interface NavSection {
  label: string;
  items: NavItem[];
}

export const nav: NavSection[] = [
  {
    label: 'Getting Started',
    items: [
      { text: 'Installation', link: '/guide/getting-started' },
      { text: 'Why Orca?', link: '/comparison' },
      { text: 'Configuration', link: '/guide/configuration' },
      { text: 'Services', link: '/guide/services' },
      { text: 'DevOps Guide', link: '/guide/devops' },
    ],
  },
  {
    label: 'Operations',
    items: [
      { text: 'Deployment', link: '/guide/deployment' },
      { text: 'Multi-Node', link: '/guide/multi-node' },
      { text: 'Monitoring', link: '/guide/monitoring' },
    ],
  },
  {
    label: 'AI Ops',
    items: [
      { text: 'AI Assistant', link: '/guide/ai-ops' },
    ],
  },
  {
    label: 'Reference',
    items: [
      { text: 'CLI Commands', link: '/reference/cli' },
      { text: 'REST API', link: '/reference/api' },
      { text: 'Self-Healing', link: '/reference/self-healing' },
    ],
  },
  {
    label: 'Architecture',
    items: [
      { text: 'Overview', link: '/architecture' },
    ],
  },
];

export function flatNav(): NavItem[] {
  return nav.flatMap(s => s.items);
}

export function prevNext(slug: string): { prev?: NavItem; next?: NavItem } {
  const base = '/orca';
  const flat = flatNav();
  const idx = flat.findIndex(item => base + item.link === slug || item.link === slug);
  return {
    prev: idx > 0 ? flat[idx - 1] : undefined,
    next: idx < flat.length - 1 ? flat[idx + 1] : undefined,
  };
}
