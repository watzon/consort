# The Consort hub as a container image.
#
#   docker build --build-context sayso=../sayso -t consort-hub .
#   docker run -d --name consort-hub -p 127.0.0.1:7600:7600 \
#     -v consort-hub-data:/data -e CONSORT_PUBLIC_URL=https://hub.example.com \
#     consort-hub
#
# The image holds the whole `consort` program. Its entry point runs the hub.
# Put a reverse proxy in front for HTTPS (see docs/deploy.md).

FROM rust:1.98-bookworm AS build
WORKDIR /src/consort
# Only the CLI and its crates build here. The desktop app needs system
# libraries that a server image does not have.
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY design ./design
# The workspace has a path dependency on a Sayso checkout beside this repo.
COPY --from=sayso --exclude=target --exclude=.git . /src/sayso
RUN cargo build --release --locked -p consort-cli

FROM debian:bookworm-slim
# git is for skill sources. curl is for the health check.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /data --shell /usr/sbin/nologin consort \
    && mkdir /data \
    && chown consort:consort /data
COPY --from=build /src/consort/target/release/consort /usr/local/bin/consort
USER consort
ENV CONSORT_HUB_DATA=/data \
    CONSORT_HUB_LISTEN=0.0.0.0:7600 \
    CONSORT_INSTALL_KIND=docker
VOLUME /data
EXPOSE 7600
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD curl -fsS http://127.0.0.1:7600/healthz || exit 1
ENTRYPOINT ["consort", "hub"]
