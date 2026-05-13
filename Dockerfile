
FROM rust:1.95-alpine AS build

ARG FB2C_VERSION=v1.78.5

ARG TARGETARCH
ARG TARGETOS


WORKDIR     /build

RUN         apk add --update --no-cache ca-certificates curl openssl-dev openssl-libs-static musl-dev

ADD         . /build

RUN         mkdir -p /build/bin && cd /build && \
            cargo build --release && \
            curl -L "https://github.com/rupor-github/fb2converter/releases/download/${FB2C_VERSION}/fb2c-${TARGETOS}-${TARGETARCH}.zip" -o fb2c-${TARGETOS}-${TARGETARCH}.zip && \
            unzip -d target/release/ fb2c-${TARGETOS}-${TARGETARCH}.zip && \
            rm -rf fb2c-${TARGETOS}-${TARGETARCH}.zip && \
            echo Done!

FROM alpine:3.23

RUN         apk add --update --no-cache ca-certificates

WORKDIR     /flibooks

COPY        --from=build /build/target/release/flibooks-es /flibooks/
COPY        --from=build /build/log4rs.yml /flibooks/
COPY        --from=build /build/target/release/fb2c /flibooks/

EXPOSE      8000

ENTRYPOINT [ "/flibooks/flibooks-es" ]
