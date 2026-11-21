FROM rust:1.94.0-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
COPY training/ training/
RUN sh training/build.sh

FROM debian:bookworm-slim
RUN useradd --uid 1000 --no-create-home --home-dir /scratch app
COPY --from=build /build/target/release/shopping-sessions /usr/local/bin/shopping-sessions
USER 1000:1000
CMD ["/usr/local/bin/shopping-sessions"]
