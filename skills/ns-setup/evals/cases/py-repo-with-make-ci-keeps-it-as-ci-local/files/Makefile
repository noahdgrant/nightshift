.PHONY: ci

# Everything CI runs, in order. Takes no flags.
ci:
	ruff check .
	ruff format --check .
	python3 -m pytest -q
