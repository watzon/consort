# The Consort session host. Use it with deploy/session-host/seccomp.json. A
# session does not start when bubblewrap cannot run. The preview browser has the
# same need: with the default seccomp profile of Docker, it stops with "No usable
# sandbox!", and a visual has no image.
#
# The workspace has a path dependency on a Sayso checkout beside this repo, so
# the build has a second context. Build from the root of this repo:
#   docker build --build-context sayso=../sayso \
#     -f deploy/session-host/Dockerfile -t consort-session-host .
FROM rust:1.98-slim-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev git ca-certificates cmake clang
WORKDIR /src/consort
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY design ./design
COPY --from=sayso --exclude=target --exclude=.git . /src/sayso
RUN cargo build --release --locked -p consort-cli --bin consort

FROM debian:bookworm-slim
# The libraries from libasound2 to libxrandr2, fontconfig and fonts-liberation
# are for the preview browser (chrome-headless-shell). The session host
# downloads the browser on first use, and draws the image of each visual with
# it. Without fontconfig, sans-serif text shows in a monospace font. Without
# fonts-liberation, a page has no glyphs.
RUN apt-get update \
  && apt-get install -y --no-install-recommends bubblewrap git ca-certificates curl \
    libasound2 libatk-bridge2.0-0 libatk1.0-0 libatspi2.0-0 libdbus-1-3 libexpat1 \
    libgbm1 libglib2.0-0 libnspr4 libnss3 libx11-6 libxcb1 libxcomposite1 \
    libxdamage1 libxext6 libxfixes3 libxkbcommon0 libxrandr2 \
    fontconfig fonts-liberation \
  && rm -rf /var/lib/apt/lists/* \
  && useradd --create-home --home-dir /home/consort --shell /usr/sbin/nologin consort \
  && mkdir -p /data && chown consort:consort /data
COPY --from=build /src/consort/target/release/consort /usr/local/bin/consort
USER consort
ENV HOME=/home/consort
VOLUME /data
ENTRYPOINT ["/usr/local/bin/consort", "session-host"]
CMD ["run", "--config-dir", "/data/config", "--consort-home", "/data/home"]
