FROM rustlang/rust:nightly as builder

RUN apt-get update && apt-get -y upgrade && mkdir /tmp/build
WORKDIR /tmp/build

# ffmpeg build deps
# RUN apt-get install -y build-essential curl tar libass-dev libtheora-dev libvorbis-dev libtool cmake automake autoconf

# statically compiled ffmpeg. debian's ffmpeg comes with the everything (~800 mb image)
RUN apt-get install -y curl xz-utils && \
    curl https://johnvansickle.com/ffmpeg/releases/ffmpeg-release-amd64-static.tar.xz | tar -xJf - && \
    bash -c "mv ./ffmpeg-*/ffmpeg /tmp/build && rm -rf ./ffmpeg-*/" && \
    apt-get remove -y xz-utils curl

# Rust build deps
RUN apt-get install -y protobuf-compiler libssl-dev

ADD Cargo.toml /tmp/build
RUN cargo build --release || echo "ok"

ADD src /tmp/build/src
ADD build.rs /tmp/build
ADD proto /tmp/build/proto

ENV PROTOC="/usr/bin/protoc"
ENV PROTO_PATH="/tmp/build/proto"
RUN cargo build --release
RUN cp target/release/vredditor-api . && cargo clean

FROM debian:bullseye-slim

RUN apt-get update && \
    apt-get -y upgrade && \
    apt-get install --no-install-recommends -y tini python3 python3-pip && \
    rm -rf /tmp/*

RUN pip3 install --no-cache-dir youtube_dl

COPY --from=builder /tmp/build/ffmpeg /tmp/build/vredditor-api /bin/

RUN mkdir /scratch && chmod +x /bin/vredditor-api

VOLUME /scratch

ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["/bin/vredditor-api"]
