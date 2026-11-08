FROM debian:trixie-slim AS fetch
ARG PROWLARR_URL=https://github.com/Prowlarr/Prowlarr/releases/download/v2.7.0.5723/Prowlarr.master.2.7.0.5723.linux-core-x64.tar.gz
ARG PROWLARR_SHA256=d65cb9fb442a8a3ded055d7027db9a08f519acb7908c7afe72175aa7c337783c
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
 && curl -fsSL -o /prowlarr.tar.gz "$PROWLARR_URL" \
 && echo "$PROWLARR_SHA256  /prowlarr.tar.gz" | sha256sum -c - \
 && mkdir /app && tar xzf /prowlarr.tar.gz -C /app --strip-components=1 \
 && rm -rf /app/Prowlarr.Update

FROM debian:trixie-slim
ARG UID=1000
ARG GID=1000
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libicu76 libsqlite3-0 && rm -rf /var/lib/apt/lists/* \
 && groupadd --gid ${GID} prowlarr \
 && useradd --uid ${UID} --gid ${GID} --no-create-home --home-dir /config --shell /usr/sbin/nologin prowlarr \
 && { find / -xdev -perm /6000 -type f -exec chmod a-s {} + 2>/dev/null || true; }
COPY --from=fetch /app /app
ENV HOME=/config
USER prowlarr:prowlarr
VOLUME ["/config"]
EXPOSE 9696
ENTRYPOINT ["/app/Prowlarr", "-nobrowser", "-data=/config"]
