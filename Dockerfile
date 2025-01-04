# NOTE: This Dockerfile depends on you building the analytics binary first.
# It will then package that binary into the image, and use that as the entrypoint.
# This means that running `docker build` is not a repeatable way to build the same
# image, but the benefit is much faster cross-platform builds; a net win.
FROM debian:bookworm-slim

LABEL org.opencontainers.image.source=https://github.com/SierraSoftworks/analytics
LABEL org.opencontainers.image.description="Lightweight and privacy preserving analytics for your website(s)."
LABEL org.opencontainers.image.licenses=MIT

RUN apt-get update && apt-get install -y \
  openssl

ADD ./analytics /usr/local/bin/analytics

ENTRYPOINT ["/usr/local/bin/analytics"]
