# Tiiny Studio server (Rust, tiiny-studio-server) in a small image. compose.yaml builds this.
# By hand:  docker build -t tiiny-studio .
# Base images are pinned by digest (multi-arch index; the tag names the version): update them on purpose.
FROM rust:1.97-slim-bookworm@sha256:2775a09d208ff0d7c1f50490c45b62db929e87ba1dcbc3f2132ac71a704bcdd3 AS build
WORKDIR /src
COPY rust ./rust
COPY static ./static
COPY docs ./docs
COPY VERSION ./
RUN cd rust && cargo build --release --locked -p studio-server

FROM debian:bookworm-slim@sha256:99c0b25c46d0214d5a2091092e7908907dcaecd6a962cde0ac700acee222b6f3
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/rust/target/release/tiiny-studio-server /usr/local/bin/tiiny-studio-server
COPY LICENSE NOTICE /usr/share/doc/tiiny-studio/
# Inside the container the server must listen on all interfaces; compose's port binding
# (STUDIO_BIND, default 127.0.0.1) decides who can actually reach it. Starts as root only to switch to the data folder's
# owner (or uid 10001 when root owns it); it never keeps root.
ENV PORT=7860 DATA_DIR=/data HOST=0.0.0.0 STUDIO_DOCKER=1
VOLUME /data
EXPOSE 7860
CMD ["tiiny-studio-server"]
