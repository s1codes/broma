ARG CROSS_BASE_IMAGE
FROM ${CROSS_BASE_IMAGE}

RUN dpkg --add-architecture arm64 \
    && apt-get update \
    && apt-get install --assume-yes --no-install-recommends libasound2-dev:arm64 \
    && rm -rf /var/lib/apt/lists/*
