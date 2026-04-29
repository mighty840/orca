import { defineConfig } from 'astro/config';
import rehypeRaw from 'rehype-raw';

const ICONS = { tip: '💡', warning: '⚠️', danger: '🚨' };

function getParaText(node) {
  if (node.type === 'paragraph' && node.children?.[0]?.type === 'text') {
    return node.children[0].value.trim();
  }
  return null;
}

function remarkVitePressContainers() {
  return function (tree) {
    const src = tree.children;
    const out = [];
    let i = 0;

    while (i < src.length) {
      const node = src[i];
      const text = getParaText(node);
      const m = text?.match(/^:::\s*(tip|warning|danger|code-group)(?:\s+(.*))?$/);

      if (m) {
        const type = m[1];
        const title = (m[2] || '').trim();
        const body = [];
        i++;

        while (i < src.length) {
          const inner = src[i];
          const innerText = getParaText(inner);
          if (innerText === ':::') { i++; break; }
          body.push(inner);
          i++;
        }

        if (type === 'code-group') {
          out.push({ type: 'html', value: '<div class="code-group">' });
          out.push(...body);
          out.push({ type: 'html', value: '</div>' });
        } else {
          const icon = ICONS[type] || 'ℹ️';
          const label = title || type.charAt(0).toUpperCase() + type.slice(1);
          out.push({ type: 'html', value: `<div class="callout callout-${type}"><div class="callout-label">${icon} ${label}</div>` });
          out.push(...body);
          out.push({ type: 'html', value: '</div>' });
        }
      } else {
        out.push(node);
        i++;
      }
    }

    tree.children = out;
  };
}

export default defineConfig({
  base: '/orca',
  outDir: './dist',
  markdown: {
    shikiConfig: {
      theme: 'nord',
      wrap: false,
    },
    remarkPlugins: [remarkVitePressContainers],
    rehypePlugins: [rehypeRaw],
    remarkRehype: { allowDangerousHtml: true },
  },
});
