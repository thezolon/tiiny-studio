# Tiiny Studio server (Rust, tiiny-studio-server) in a small image. compose.yaml builds this.
# By hand:  docker build -t tiiny-studio .
FROM rust:1-slim-bookworm AS build
WORKDIR /src
COPY rust ./rust
COPY static ./static
COPY docs ./docs
COPY VERSION ./
RUN cd rust && cargo build --release --locked -p studio-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/rust/target/release/tiiny-studio-server /usr/local/bin/tiiny-studio-server
COPY LICENSE NOTICE /usr/share/doc/tiiny-studio/
# Inside the container the server must listen on all interfaces; compose's port binding
# (STUDIO_BIND, default 127.0.0.1) decides who can actually reach it. Runs as the data folder's owner on Linux.
ENV PORT=7860 DATA_DIR=/data HOST=0.0.0.0
VOLUME /data
EXPOSE 7860
CMD ["tiiny-studio-server"]
