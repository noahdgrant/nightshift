feat(api): POST /orders creates an order

`POST /orders` stores an order for the caller and returns 201 with its id. It is a network service endpoint that clients on flaky networks call, and they retry on a timeout.
