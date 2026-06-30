# TODO: base image + build for komga. Mirror jellyfin/Dockerfile conventions.
FROM debian:12-slim
LABEL org.opencontainers.image.source="https://github.com/argyle-labs/komga"
EXPOSE 25600
