# Performance

Two tools to make a page faster: render times on every response, and the profile of one page.
Both measure local-storefront on your machine, not Shopify. Use them to compare: before and
after a change, one section with another. To see what a page would cost Shopify, profile it in
[points](#what-the-page-would-cost-shopify).

## Profile a page

```bash
npx local-storefront profile /collections/all
```

```text
/collections/all · template collection · 2.502 ms
The render in the middle of 15, which took from 2.325 to 2.901 ms.
Times are in ms, for all the calls of a row. "own" leaves out what is under the row.

   total      own  calls
   2.502    0.201      1  render
   1.510    0.005      1    templates/collection.json
   1.464    0.029      1      section template--8034641070597__main
   1.435    0.031      1        sections/main-collection
   0.921    0.021      8          block product-card
   0.900    0.031      8            blocks/_product-card
   0.495    0.010      8              block card-gallery
   0.485    0.022      8                blocks/_product-card-gallery
   0.398    0.112      8                  snippets/card-gallery
   0.122    0.122     15                    snippets/product-media

Slowest on their own:

     own  calls
   0.191     12  snippets/list-filter
   0.115     15  snippets/product-media
```

Each row is under the row that rendered it: a section, its file, its blocks, their snippets.

- `total` is the time of the row and of everything under it.
- `own` is the time of the row alone.
- `calls` is how many times it was rendered. Both times are for all the calls.
- `section <id>` and `block <key>` are a section and a block as the template places them:
  reading their settings, then their file, which is the row under them.

The page is rendered 3 times first, so that reading the files does not count. The profile is
the one of the render in the middle, by how long they took.

### The slow line of a file

```bash
npx local-storefront profile /collections/all --lines
```

Rows such as `snippets/product-media:40` appear: a tag or an output, by its line. Timing each
one makes the render slower: compare the rows with each other.

### In the browser

With the server running, the same report is a page:

```text
http://127.0.0.1:9292/__lsf/profile?path=/collections/all
```

It profiles the page as your session sees it: its cart, its customer. Add `html=1` for a
flame graph:

```text
http://127.0.0.1:9292/__lsf/profile?path=/collections/all&html=1
```

This is speedscope, the viewer `shopify theme profile` opens. local-storefront embeds it:
nothing is fetched from the internet. The name of the profile, at the top, switches between time
and points. The status page (`/__lsf`) links every page to its profiles.

Without a server, write the flame graph to a file and drop it on <https://www.speedscope.app>:

```bash
npx local-storefront profile /collections/all --json > profile.json
```

### Options

| Option | In the URL | What it does |
|---|---|---|
| `--points` | `points=1` | Counts in points instead of time. In the flame graph, starts on points. |
| `--cost <kind>=<points>,...` | `cost=<kind>=<points>,...` | Changes what a kind of thing costs. |
| | `html=1` | Shows the flame graph. |
| `--lines` | `lines=1` | Adds every tag and every output, by file and line. Points do not change. |
| `--all` | `all=1` | Shows every row. Without it, what took less than a hundredth of the render is summed up as `... 3 more`. |
| `--runs <n>` | `runs=<n>` | Renders the page `n` times. Default: 15. |
| `--section-id <id>` | `section_id=<id>` | Profiles one section alone. |
| `--json` | `format=speedscope` | Gives the file of the flame graph instead of the report. It holds both profiles. |

## What the page would cost Shopify

local-storefront loads a product or a metafield in no time. A Shopify storefront does not: a
page that is fast here can be slow there. `--points` counts what the page asks for, instead of
the time it takes:

```bash
npx local-storefront profile /collections/all --points
```

```text
/collections/all · template collection · 15564 points

   total      own  calls
   15564        0      1  render
   11556        0      1    templates/collection.json
   11251        0      1      section template--8034641070597__main
   11251      870      1        sections/main-collection
    6865        0      8          block product-card
    6865      112      8            blocks/_product-card

What the points are made of:

  points   count   each
   14244   14244      1  liquid      A tag or an output rendered.
     800       8    100  product     A product loaded.
     220      22     10  variant     A variant loaded.
     100       1    100  collection  A collection loaded.
     200       2    100  menu        A menu loaded.
```

The report is the same tree, in points. A row has the points of everything that happened
under it: a snippet that reads a metafield of 50 products has the points of 50 metafields.

| Kind | Points | Charged for |
|---|---|---|
| `liquid` | 1 | A tag or an output rendered. |
| `product` | 100 | A product loaded. |
| `variant` | 10 | A variant loaded. |
| `collection` | 100 | A collection loaded. |
| `metafield` | 100 | A metafield read. |
| `metaobject` | 100 | A metaobject loaded. |
| `page` | 100 | A page loaded. |
| `blog` | 100 | A blog loaded. |
| `article` | 100 | An article loaded. |
| `menu` | 100 | A menu loaded. |
| `search` | 1000 | A search, a predictive search or the recommendations of a product. |

- **Points are a model, not a measure.** Shopify does not publish what it spends on what. The
  unit is a tag rendered, and what a store has to fetch is taken to weigh as much as 100
  tags. Read the counts first: they are facts. The points only weigh them.
- What is loaded counts once per page, however many times the templates read it. A product in
  a loop counts once per product.
- A metafield that does not exist counts too: the store looks for it all the same.
- Points are the same at every render and on every machine, so a test can compare them.

To change the costs, for example after comparing with `shopify theme profile` on your store:

```bash
npx local-storefront profile /collections/all --points --cost product=300,metafield=50
```

## Render times

Every response says how long the server took, in the `Server-Timing` header, as Shopify does:

```text
server-timing: processing;dur=3.09, render;dur=2.507, template;dur=1.27, layout;dur=0.986, compress;dur=0.53
```

In Chrome: DevTools, Network, click the page, Timing. The times are under "Server Timing".

To see each section and each block too:

```bash
npx local-storefront serve --timings
```

```text
section;dur=1.922;desc="main-collection template--8034641070597__main"
block;dur=0.636;desc="- filters"
block;dur=1.123;desc="- _product-card product-card x8"
block;dur=0.611;desc="- - _product-card-gallery card-gallery x8"
```

A block comes under its section, with one dash per level. `x8` means it was rendered 8 times,
and the time is the time of all 8.

In a test:

```ts
const response = await page.goto('/');
const timing = response.headers()['server-timing'];
```

### Many requests

Start from zero:

```bash
curl -X DELETE http://127.0.0.1:9292/__lsf/timings
```

Load your pages, then read the totals:

```bash
curl http://127.0.0.1:9292/__lsf/timings
```

```json
{
  "templates": [
    { "template": "collection", "count": 2000, "total": 5792.9, "mean": 2.896, "p50": 2.851, "p95": 3.31, "min": 2.308, "max": 7.495 }
  ],
  "sections": [
    { "id": "template--8034641070597__main", "type": "main-collection", "count": 2000, "total": 3455.621, "mean": 1.728, "p50": 1.688, "p95": 1.982, "min": 1.281, "max": 5.537 }
  ],
  "blocks": [
    { "id": "A1456318a0478e95__product-card", "type": "_product-card", "section": "template--8034641070597__main", "count": 16000, "total": 2196.216, "mean": 0.137, "p50": 0.12, "p95": 0.223, "min": 0.064, "max": 2.549 }
  ]
}
```

What took the most time in all comes first. A block counts once each time it is rendered: 8
product cards on 2000 pages are 16000. `p50` and `p95` are within 2% of the exact value.

### Entries of `Server-Timing`

Times are in milliseconds.

| Entry | Meaning |
|---|---|
| `processing` | Everything the server did for the request. Shopify uses the same name. |
| `render` | Rendering the Liquid of a page or of sections. |
| `template` | The template of the page: what goes into `content_for_layout`. |
| `layout` | The layout around it, with its header and its footer. |
| `section` | One section, with `--timings`. Its description is its type, then its id: the one of `#shopify-section-<id>`. |
| `block` | One theme block, with `--timings`. Its description is a dash per level, its type, its key in the template, then `x<n>` when it was rendered more than once. |
| `omitted` | Only on a page with hundreds of blocks: how many of the fastest ones the header leaves out. `/__lsf/timings` has them all. |
| `compress` | Compressing the response. |
| `throttle` | The delay of `--throttle`. It is not part of `processing`. |

- A time includes what is inside: a section is part of `template` or of `layout`, a block is
  part of its section, and of the block it is in.
- Only theme blocks are timed: the files of `blocks/`, rendered with `content_for`. Blocks
  that a section writes itself, in a loop over `section.blocks`, are part of the section.
- `/__lsf/timings` counts pages by template, sections and blocks by id. Sections and blocks
  count wherever they are rendered: in a page, alone, or in a cart response. The id of a
  block is the one of `#shopify-block-<id>`, and ends with its key.
