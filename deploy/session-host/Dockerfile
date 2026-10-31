# The Consort session host. Use it with deploy/session-host/seccomp.json. A
# session does not start when bubblewrap cannot run.
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
RUN apt-get update \
  && apt-get install -y --no-install-recommends bubblewrap git ca-certificates curl \
  && useradd --create-home --home-dir /home/consort --shell /usr/sbin/nologin consort \
  && mkdir -p /data && chown consort:consort /data
COPY --from=build /src/consort/target/release/consort /usr/local/bin/consort
USER consort
ENV HOME=/home/consort
VOLUME /data
ENTRYPOINT ["/usr/local/bin/consort", "session-host"]
CMD ["run", "--config-dir", "/data/config", "--consort-home", "/data/home"]
