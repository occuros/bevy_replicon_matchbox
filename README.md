# Bevy Replicon Matchbox

This crate integrates [`matchbox`](https://github.com/johanhelsing/matchbox) as a backend for [`bevy_replicon`](https://github.com/simgine/bevy_replicon), enabling multiplayer experiences which only need a signaling server to work.

Matchbox provides convenient NAT traversal support out of the box — no need to manually manage signaling, host discovery, or ICE negotiation.

> ⚠️ **Note**: This is an early implementation and may still contain bugs or limitations.

---

## Running an Example

Two examples are ported from `bevy_replicon`: [`simple_box`](examples/simple_box.rs) and [`tic_tac_toe`](examples/tic_tac_toe.rs).

Start a host (which also acts as a player):

```bash
cargo run --example tic_tac_toe -- server
```

and connect a client in another terminal:

```bash
cargo run --example tic_tac_toe -- client
```

The `server` command starts a local matchbox signaling server, so no external service is needed to try the examples. Both examples also run locally without networking (`cargo run --example tic_tac_toe -- hotseat`, or `cargo run --example simple_box -- single-player`).

For production setups, it’s recommended to run a dedicated matchbox signaling server.



### Known Limitations

- **Empty message workaround**  
  WebRTC can silently drop empty messages. To prevent this, each message is currently prefixed with a single `byte` to ensure delivery.


- **WASM support not verified (yet)**  
  This backend has not been tested in WebAssembly environments. Compatibility is currently unverified.

## Compatible versions

| bevy | bevy_matchbox | bevy_replicon | bevy_replicon_matchbox |
|------|---------------|---------------|------------------------|
| 0.18 | 0.14          | 0.40          | 0.17                   |
| 0.16 | 0.12          | 0.34          | 0.16                   |


## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT License](LICENSE-MIT) at your option.