# a comment that mentions no suppression
import os  # noqa: F401  # re-exported for callers
count: int = "three"  # type: ignore[assignment]  # stored as text
