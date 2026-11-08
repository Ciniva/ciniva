FROM rust:1-trixie AS build
WORKDIR /src
COPY server/Cargo.toml server/Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && echo '' > src/lib.rs && cargo build --release && rm -rf src
COPY server/src ./src
COPY server/migrations ./migrations
COPY server/examples ./examples
RUN touch src/main.rs src/lib.rs && cargo build --release --bin ciniva --example hwinfo --example keyframes --example credits

FROM debian:trixie-slim
ARG FFMPEG_PKG=jellyfin-ffmpeg8
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl gnupg \
 && install -d /etc/apt/keyrings \
 && curl -fsSL https://repo.jellyfin.org/jellyfin_team.gpg.key | gpg --dearmor -o /etc/apt/keyrings/jellyfin.gpg \
 && echo "deb [signed-by=/etc/apt/keyrings/jellyfin.gpg] https://repo.jellyfin.org/debian trixie main" > /etc/apt/sources.list.d/jellyfin.list \
 && apt-get update \
 && apt-get install -y --no-install-recommends ${FFMPEG_PKG} \
 && apt-get purge -y gnupg && apt-get autoremove -y && rm -rf /var/lib/apt/lists/*
ARG UID=1000
ARG GID=1000
RUN groupadd --gid ${GID} ciniva \
 && useradd --uid ${UID} --gid ${GID} --no-create-home --home-dir /data --shell /usr/sbin/nologin ciniva \
 && install -d -o ciniva -g ciniva -m 0750 /data \
 && { find / -xdev -perm /6000 -type f -exec chmod a-s {} + 2>/dev/null || true; }
COPY --from=build /src/target/release/ciniva /usr/local/bin/ciniva
COPY --from=build /src/target/release/examples/hwinfo /usr/local/bin/ciniva-hwinfo
COPY --from=build /src/target/release/examples/keyframes /usr/local/bin/ciniva-keyframes
COPY --from=build /src/target/release/examples/credits /usr/local/bin/ciniva-credits
COPY web /web
ENV CINIVA_MEDIA=/media \
    CINIVA_DATA=/data \
    CINIVA_WEB=/web \
    CINIVA_FFMPEG=/usr/lib/jellyfin-ffmpeg/ffmpeg \
    CINIVA_FFPROBE=/usr/lib/jellyfin-ffmpeg/ffprobe \
    NVIDIA_VISIBLE_DEVICES=all \
    NVIDIA_DRIVER_CAPABILITIES=compute,video,utility
USER ciniva:ciniva
VOLUME ["/data"]
EXPOSE 8484
ENTRYPOINT ["/usr/local/bin/ciniva"]
