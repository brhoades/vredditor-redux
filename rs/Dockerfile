FROM rustlang/rust:nightly-alpine as builder

RUN apk --no-cache --no-progress upgrade && \
    apk --no-cache --no-progress add bash curl openssl-dev musl-dev protoc && \
    mkdir /tmp/build
WORKDIR /tmp/build

ENV RUSTFLAGS="-C target-feature=-crt-static"
ENV RUST_BACKTRACE=full
ENV RUST_LOG=trace

ADD Cargo.toml /tmp/build
RUN cargo build --release || echo "ok"

ADD src /tmp/build/src
ADD build.rs /tmp/build
ADD proto /tmp/build/proto

ENV PROTOC="/usr/bin/protoc"
ENV PROTO_PATH="/tmp/build/proto"
RUN cargo build --release
RUN cp target/release/vredditor-api . && cargo clean

FROM alpine:latest

RUN apk --no-cache --no-progress upgrade && \
    apk --no-cache --no-progress add bash tini libgcc ffmpeg py3-pip && \
    rm -rf /tmp/*

RUN pip install --no-cache-dir youtube_dl && apk del py3-pip && apk add --no-cache python3

COPY --from=builder /tmp/build/vredditor-api /bin

RUN chmod +x /bin/vredditor-api

ENTRYPOINT ["/sbin/tini", "--"]
