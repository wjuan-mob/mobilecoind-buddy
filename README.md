# mobilecoind-buddy

`mobilecoind-buddy` is a simple front-end over [`mobilecoind`](https://github.com/mobilecoinfoundation/mobilecoin).

It can also talk to the [`deqs`](https://github.com/mobilecoinofficial/deqs).

It is written in rust using [`egui`](https://github.com/emilk/egui). (This makes it easier to make grpc calls because
we can import the rust API crates and get all the nice type checking, compared to python.)

This is a rapid prototype meant for demos or for developer use. It isn't really meant to be a user-facing product
and may have some rough edges, particularly around starting it. However, it has been engineered with proper architecture
and error handling, and should be good at what it does.

## Quickstart

First, start `mobilecoind`. It should be listening for grpc on `localhost:4444` (the default).

You can open a new terminal and use `./build_and_run_testnet_mobilecoind.sh` if you like.

Then, you can use a command like `cargo run --release -- --keyfile=example/account_key.json` to start the front-end.

## Deqs

You can optionally specify `--deqs-uri` flag if you want to connect to a DEQS deployment, enabling swaps.

For testnet, you can use `--deqs-uri=insecure-deqs://deqs.chrisbeck.co`.
