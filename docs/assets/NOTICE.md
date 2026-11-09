# Logo assets

The nightshift logo is drawn for this project and is covered by the project's licence, except for
the letters of the wordmark, which are derived from a font under its own licence (below). The mark
is a rounded tile with a command prompt (`>_`) and a crescent moon cut out of it: the `ns` command
line, at night. Every shape of the mark is a circle, an arc, a rounded rectangle or a straight line,
drawn for this project.

## The wordmark's font

The word "nightshift" in `logo.svg`, `logo-dark.svg`, `logo-light.svg`, `logo-mono-black.svg`,
`logo-mono-white.svg` (and in `social-preview.png`) is set in **Zilla Slab Bold** (weight 700). These
files contain **outlines derived from the font**: the text was converted to SVG paths, so nothing
renders with a font and no font file is part of this repository.

- Font: Zilla Slab, Bold, `ZillaSlab-Bold.ttf`.
- Designer: Typotheque (typotheque.com), made for Mozilla.
- Copyright, as in the font's own `OFL.txt`: `Copyright 2017, The Mozilla Foundation`.
- Licence: SIL Open Font License, Version 1.1 (26 February 2007), text and FAQ at
  <https://openfontlicense.org>. The Font Software is not modified or redistributed here; the licence
  allows its use to make artwork, and the outlines are shown with this notice.
- "Zilla is a trademark of The Mozilla Corporation" (from the font's name table). Nothing here
  implies a connection with Mozilla.
- Source: the Google Fonts repository, `github.com/google/fonts`, directory `ofl/zillaslab`.
- Made with opentype.js 1.3.4: the glyphs of "nightshift" at a fixed size with the font's kerning,
  as one path for "night" and one for "shift".

## Colours and files

The logo has **one amber, `#f5b942`**, for the tile and for the letters "shift" on every background,
light or dark. The only other colours are the text colour of the letters "night": `#d6d9de` on dark
and `#1c1d20` on light. The prompt and the moon are holes in the tile, so the background shows
through them.

Contrast of the amber (WCAG 2 relative luminance): 1.76:1 on white `#ffffff`, 1.66:1 on GitHub's
light canvas `#f6f8fa`, 10.26:1 on `#151618` and 10.72:1 on GitHub's dark `#0d1117`. On light
backgrounds it is below the 3:1 that the WCAG guideline asks of a graphical object. That is a known
trade-off, to keep the same amber everywhere; the dark letters of "night" carry the name there.

| File | For | Amber | "night" |
| --- | --- | --- | --- |
| `logo-mark.svg` | any background | `#f5b942` | none |
| `logo-dark.svg` | dark backgrounds | `#f5b942` | `#d6d9de` |
| `logo-light.svg` | light backgrounds | `#f5b942` | `#1c1d20` |
| `logo.svg` | follows `prefers-color-scheme` (dark when there is none) | `#f5b942` | `#d6d9de`; `#1c1d20` in light |
| `logo-mono-black.svg`, `logo-mono-white.svg` | one colour | black, white | the same |
| `social-preview.png` | dark by design | `#f5b942` on `#151618` | `#d6d9de` |

`logo-dark.svg` and `logo-light.svg` differ only in the colour of "night". The README shows them
through a `<picture>`.

`social-preview.png` is 1280 x 640, GitHub's size for a repository social preview. It is set by hand
in the repository's settings (Settings, General, Social preview); the file does nothing until then.
It is rendered from `social-preview.html` with a headless browser at that size.
