# craftradio-server in a container. Build from the repository root:
#
#   docker build -f packaging/server/Dockerfile -t craftradio-server .
#
# Run it with a config whose `listen` is "0.0.0.0:8080" (see README.md, "Container"):
#
#   docker run -p 8080:8080 -v ./config.toml:/etc/craftradio-server/config.toml:ro craftradio-server
#
# libopus and LAME are built from source and linked in statically by their crates, so the
# runtime image needs only glibc.

ARG RUST_VERSION=1.100.0

FROM rust:${RUST_VERSION}-bookworm AS build
RUN apt-get update \
    && apt-get install -y --no-install-recommends cmake pkg-config \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p craftradio-server \
    && install -m 0755 target/release/craftradio-server /usr/local/bin/craftradio-server

FROM debian:bookworm-slim
RUN useradd --system --user-group --uid 10001 --no-create-home --shell /usr/sbin/nologin craftradio \
    && install -d -o root -g craftradio -m 0750 /etc/craftradio-server \
    && install -d -o craftradio -g craftradio -m 0750 /var/lib/craftradio-server
COPY --from=build /usr/local/bin/craftradio-server /usr/local/bin/craftradio-server
USER craftradio
VOLUME /var/lib/craftradio-server
EXPOSE 8080
ENTRYPOINT ["craftradio-server"]
CMD ["run", "--config", "/etc/craftradio-server/config.toml"]
