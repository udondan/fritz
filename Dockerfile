# Runs a static fritz binary on Alpine. By default it is built from the source;
# releases pass `--build-arg BINARY=prebuilt` and copy the musl binaries of
# the release from dist/<arch>/fritz, so no Rust is compiled under emulation.
# `--build-arg PROFILE=dev` builds without optimizations, in a fraction of the
# time.
ARG BINARY=source

FROM rust:1.100-alpine AS source

# ring needs a C compiler and the musl headers.
RUN apk add --no-cache musl-dev

ARG PROFILE=release
WORKDIR /src
COPY . .
# Cargo writes the profile dev to target/debug.
RUN cargo build --profile "$PROFILE" --locked -p fritz \
    && dir=$([ "$PROFILE" = dev ] && echo debug || echo "$PROFILE") \
    && cp "target/$dir/fritz" /usr/local/bin/fritz


FROM scratch AS prebuilt
ARG TARGETARCH
COPY dist/${TARGETARCH}/fritz /usr/local/bin/fritz


# BuildKit only builds the stage that is used.
FROM ${BINARY} AS binary


FROM alpine:3.24

LABEL com.docker.desktop.mcp="true"
LABEL com.docker.desktop.mcp.name="fritz"

# apk upgrade pulls fixes for Alpine packages released after the base image.
RUN apk upgrade --no-cache && adduser -D -H -u 10001 fritz
COPY --from=binary /usr/local/bin/fritz /usr/local/bin/fritz
USER 10001

# A container has no keychain: the password comes from FRITZ_PASSWORD, the
# device from FRITZ_HOST, FRITZ_USERNAME and FRITZ_TLS_FINGERPRINT.
ENV FRITZ_NO_KEYCHAIN=1

# `docker run udondan/fritz <command>` runs the CLI; without a command the
# image starts the MCP server, which is what the Docker MCP Toolkit expects.
ENTRYPOINT ["fritz"]
CMD ["mcp"]
