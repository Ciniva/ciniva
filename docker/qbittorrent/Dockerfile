FROM debian:trixie-slim AS fetch
ARG QBT_URL=https://github.com/userdocs/qbittorrent-nox-static/releases/download/release-5.1.4_v2.0.12/x86_64-qbittorrent-nox
ARG QBT_SHA256=e42bd8f84350adfdd690757ec650ec79b58e4806ec65438cdf9b92a2f61886ca
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
 && curl -fsSL -o /qbittorrent-nox "$QBT_URL" \
 && echo "$QBT_SHA256  /qbittorrent-nox" | sha256sum -c - \
 && chmod 0755 /qbittorrent-nox

FROM debian:trixie-slim
ARG UID=1000
ARG GID=1000
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/* \
 && groupadd --gid ${GID} qbittorrent \
 && useradd --uid ${UID} --gid ${GID} --no-create-home --home-dir /config --shell /usr/sbin/nologin qbittorrent \
 && { find / -xdev -perm /6000 -type f -exec chmod a-s {} + 2>/dev/null || true; }
COPY --from=fetch /qbittorrent-nox /usr/local/bin/qbittorrent-nox
ENV HOME=/config
USER qbittorrent:qbittorrent
VOLUME ["/config"]
EXPOSE 8080 61464 61464/udp
ENTRYPOINT ["/usr/local/bin/qbittorrent-nox", "--profile=/config", "--webui-port=8080", "--confirm-legal-notice"]
