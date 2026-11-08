FROM rust:1-trixie AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock* ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && cargo build --release && rm -rf src
COPY src ./src
RUN touch src/main.rs && cargo build --release

FROM debian:trixie-slim
ARG UID=1000
ARG GID=1000
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/* \
 && groupadd --gid ${GID} plugin \
 && useradd --uid ${UID} --gid ${GID} --no-create-home --shell /usr/sbin/nologin plugin \
 && { find / -xdev -perm /6000 -type f -exec chmod a-s {} + 2>/dev/null || true; }
COPY --from=build /src/target/release/ciniva-qbittorrent /usr/local/bin/ciniva-qbittorrent
USER plugin:plugin
EXPOSE 8090
ENTRYPOINT ["/usr/local/bin/ciniva-qbittorrent"]
