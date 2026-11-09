"""Save a YouTube video's transcript as a Markdown file.

    python -I fetch.py <youtube url or 11-character id> --out <file.md> [--lang en]

Needs the `youtube-transcript-api` package (see SKILL.md). Everything else is the standard library.
"""

import argparse
import datetime
import json
import re
import sys
import urllib.parse
import urllib.request

ID = re.compile(r"^[A-Za-z0-9_-]{11}$")
# A timestamp is written at the start of a paragraph about this many seconds long.
PARAGRAPH_SECONDS = 60


def video_id(source: str) -> str:
    """The 11-character id in a youtu.be, watch, shorts, embed or live link, or in `source` itself."""
    if ID.match(source):
        return source
    url = urllib.parse.urlparse(source)
    host = url.netloc.removeprefix("www.").removeprefix("m.")
    if host == "youtu.be":
        found = url.path.strip("/").split("/")[0]
    elif host in ("youtube.com", "music.youtube.com"):
        query = urllib.parse.parse_qs(url.query)
        parts = url.path.strip("/").split("/")
        found = query["v"][0] if "v" in query else parts[-1] if parts[0] in ("shorts", "embed", "live") else ""
    else:
        found = ""
    if not ID.match(found):
        sys.exit(f"fetch.py: no YouTube video id in {source!r}")
    return found


def details(id: str) -> tuple[str, str]:
    """The video's title and channel from YouTube's oEmbed endpoint; blanks when it will not say."""
    address = "https://www.youtube.com/oembed?format=json&url=" + urllib.parse.quote(
        f"https://www.youtube.com/watch?v={id}", safe=""
    )
    try:
        with urllib.request.urlopen(address, timeout=20) as reply:
            data = json.load(reply)
        return data.get("title", ""), data.get("author_name", "")
    except Exception:
        return "", ""


def stamp(seconds: float) -> str:
    s = int(seconds)
    return f"{s // 3600}:{s % 3600 // 60:02d}:{s % 60:02d}" if s >= 3600 else f"{s // 60}:{s % 60:02d}"


def paragraphs(snippets) -> list[str]:
    """The snippets joined into paragraphs, each starting with the time it begins at."""
    out, words, began = [], [], 0.0
    for snippet in snippets:
        if words and snippet.start - began >= PARAGRAPH_SECONDS:
            out.append(f"[{stamp(began)}] {' '.join(words)}")
            words, began = [], snippet.start
        words.append(" ".join(snippet.text.split()))
    if words:
        out.append(f"[{stamp(began)}] {' '.join(words)}")
    return out


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("source", help="a YouTube link or an 11-character video id")
    parser.add_argument("--out", required=True, help="the Markdown file to write")
    parser.add_argument("--lang", default="en", help="the transcript's language code (default en)")
    args = parser.parse_args()

    try:
        from youtube_transcript_api import YouTubeTranscriptApi
    except ImportError:
        sys.exit("fetch.py: the youtube-transcript-api package is missing; see SKILL.md, step 1")

    id = video_id(args.source)
    try:
        snippets = list(YouTubeTranscriptApi().fetch(id, languages=[args.lang]))
    except Exception as e:  # the library has one error class for each cause; its message says which
        sys.exit(f"fetch.py: no transcript for {id}: {type(e).__name__}: {str(e).strip().splitlines()[0]}")

    title, channel = details(id)
    body = paragraphs(snippets)
    head = [
        f"# {title or id}",
        "",
        f"- Source: https://www.youtube.com/watch?v={id}",
        f"- Channel: {channel or 'unknown'}",
        f"- Language: {args.lang}",
        f"- Fetched: {datetime.date.today().isoformat()} with youtube-transcript-api",
        "",
    ]
    with open(args.out, "w", encoding="utf-8") as f:
        f.write("\n".join(head) + "\n" + "\n\n".join(body) + "\n")
    words = sum(len(p.split()) for p in body)
    print(f"wrote {args.out}: {len(snippets)} snippets, {words} words, title {title!r}")


if __name__ == "__main__":
    main()
