// The documentation site: the guides of `docs/`, rendered as they are written. The files are the
// ones GitHub shows and some tests read, so nothing here asks them to change: the title of a page
// is its first heading, the sidebar is the index of `docs/README.md`, the anchors are GitHub's,
// and a link out of `docs/` leads to the file on GitHub.
import { copyFileSync, readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { slug } from 'github-slugger'
import { defineConfig, type DefaultTheme } from 'vitepress'

const repository = 'https://github.com/0xtlt/local-storefront'
const docs = fileURLToPath(new URL('../../../docs/', import.meta.url))
// `llms.txt`, the index an AI agent reads first, is at the root of the repository: the site
// serves it at `/llms.txt` too.
const llms = fileURLToPath(new URL('../../../llms.txt', import.meta.url))
const site = fileURLToPath(new URL('../', import.meta.url))

/** The sidebar: each `## Section` of `docs/README.md`, with the pages its table links to. */
function sidebar(): DefaultTheme.SidebarItem[] {
  const groups: { text: string; items: DefaultTheme.SidebarItem[] }[] = []
  for (const line of readFileSync(`${docs}README.md`, 'utf8').split('\n')) {
    const heading = line.match(/^## (.+)/)
    if (heading) {
      groups.push({ text: heading[1], items: [] })
      continue
    }
    for (const [, text, page] of line.matchAll(/\[([^\]]+)\]\(([a-z0-9-]+)\.md\)/g)) {
      groups.at(-1)?.items.push({ text, link: `/${page}` })
    }
  }
  return groups.filter((group) => group.items.length > 0)
}

export default defineConfig({
  title: 'local-storefront',
  description: 'Run a Shopify theme, the checkout of an app and its Admin API on your machine, without Shopify.',
  srcDir: '../../docs',
  // `docs/README.md` is the index of the guides, on GitHub as here.
  rewrites: { 'README.md': 'index.md' },
  markdown: {
    // The anchors GitHub gives headings, so that `cli.md#app-webhook-trigger` works on both.
    anchor: { slugify: (text) => slug(text) },
    config(md) {
      // The guides show Liquid (`{{ product.title }}`), which Vue would read as its own
      // templates: a page is HTML to show, not a component.
      md.core.ruler.push('lsf_v_pre', (state) => {
        const open = new state.Token('html_block', '', 0)
        open.content = '<div v-pre>\n'
        const close = new state.Token('html_block', '', 0)
        close.content = '</div>\n'
        state.tokens.unshift(open)
        state.tokens.push(close)
      })
      // A link out of `docs/` (`../examples/checkout-playground`) leads to the file on GitHub.
      const open = md.renderer.rules.link_open
      md.renderer.rules.link_open = (tokens, index, options, env, self) => {
        const href = tokens[index].attrGet('href')
        if (href?.startsWith('../')) {
          tokens[index].attrSet('href', `${repository}/tree/main/${href.slice(3)}`)
        }
        return open ? open(tokens, index, options, env, self) : self.renderToken(tokens, index, options)
      }
    },
  },
  themeConfig: {
    sidebar: sidebar(),
    outline: { level: [2, 3] },
    search: { provider: 'local' },
    socialLinks: [{ icon: 'github', link: repository }],
    editLink: {
      // Sent to the browser as its source: it cannot name `repository`.
      pattern: ({ filePath }) => `https://github.com/0xtlt/local-storefront/edit/main/docs/${filePath}`,
      text: 'Edit this page on GitHub',
    },
  },
  buildEnd(site) {
    copyFileSync(llms, `${site.outDir}/llms.txt`)
  },
  vite: {
    // `docs/` is outside this folder: the dev server may read it,
    server: { fs: { allow: ['../..'] } },
    plugins: [
      {
        name: 'lsf-llms-txt',
        configureServer(server) {
          server.middlewares.use('/llms.txt', (_request, response) => {
            response.setHeader('content-type', 'text/plain; charset=utf-8')
            response.end(readFileSync(llms))
          })
        },
      },
      {
        // and what its pages import (`vue`, …) is in this folder's `node_modules`, not above them.
        name: 'lsf-docs-imports',
        enforce: 'pre',
        resolveId(source, importer, options) {
          const bare = !/^[./\0]/.test(source)
          if (!bare || !importer?.startsWith(docs)) return null
          return this.resolve(source, `${site}package.json`, { ...options, skipSelf: true })
        },
      },
    ],
  },
})
