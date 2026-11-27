# Build from the repository root (the server needs the workspace):
#   docker build -f server/Dockerfile -t primeview-server .
#   docker run -p 8787:8787 -v primeview-data:/data primeview-server
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p primeview-server

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home /data primeview \
    && mkdir -p /data && chown primeview /data
COPY --from=build /src/target/release/primeview-server /usr/local/bin/primeview-server
USER primeview
ENV PRIMEVIEW_BIND=0.0.0.0:8787 \
    PRIMEVIEW_DATA=/data/alerts.json
VOLUME ["/data"]
EXPOSE 8787
ENTRYPOINT ["/usr/local/bin/primeview-server"]
