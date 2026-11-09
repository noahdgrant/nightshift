**Changed**: added `GET /products/{id}` with an in-process cache in front of the database.

**Tests**: `get_product_returns_it` and `get_missing_product_is_404` in `tests/products.rs`.
