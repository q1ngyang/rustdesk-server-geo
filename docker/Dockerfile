FROM alpine:3.22

RUN apk add --no-cache ca-certificates curl

COPY --chmod=755 hbbs /usr/bin/hbbs
COPY --chmod=755 hbbr /usr/bin/hbbr
COPY --chmod=755 rustdesk-utils /usr/bin/rustdesk-utils
COPY --chmod=755 geo-entrypoint.sh /usr/local/bin/geo-entrypoint.sh

ENV HOME=/root \
    GEO_RELAY_ENABLED=true \
    GEOIP_DB_PATH=/root/geoip/GeoLite2-Country.mmdb \
    GEOIP_UPDATE_INTERVAL=86400 \
    GEOIP_UPDATE_ON_START=true \
    GEOIP_DOWNLOAD_TIMEOUT=180 \
    GEOIP_MIN_BYTES=65536

WORKDIR /root
VOLUME ["/root"]

EXPOSE 21115 21116 21116/udp 21117 21118 21119

ENTRYPOINT ["/usr/local/bin/geo-entrypoint.sh"]
