// Copyright (c) 2018-2022 The MobileCoin Foundation

//! Extension traits that make it easier to start GRPC servers and connect to
//! them using URIs.

use grpcio::{Channel, ChannelBuilder, ChannelCredentialsBuilder, Environment};
use mc_util_uri::ConnectionUri;
use std::{sync::Arc, time::Duration};
use tracing::{event, Level};

/// A trait to ease grpcio channel construction from URIs.
pub trait ConnectionUriGrpcioChannel {
    /// Construct a ChannelBuilder with some sane defaults.
    fn default_channel_builder(env: Arc<Environment>) -> ChannelBuilder {
        ChannelBuilder::new(env)
            .keepalive_permit_without_calls(true)
            .keepalive_time(Duration::from_secs(10))
            .keepalive_timeout(Duration::from_secs(20))
            .max_reconnect_backoff(Duration::from_millis(2000))
            .initial_reconnect_backoff(Duration::from_millis(1000))
    }

    /// Connects a ChannelBuilder using a URI.
    fn connect_to_uri(self, uri: &impl ConnectionUri) -> Channel;
}

impl ConnectionUriGrpcioChannel for ChannelBuilder {
    fn connect_to_uri(mut self, uri: &impl ConnectionUri) -> Channel {
        if uri.use_tls() {
            if let Some(host_override) = uri.tls_hostname_override() {
                self = self.override_ssl_target(host_override);
            }

            let creds = match uri.ca_bundle().expect("failed getting ca bundle") {
                Some(cert) => ChannelCredentialsBuilder::new().root_cert(cert).build(),
                None => ChannelCredentialsBuilder::new().build(),
            };

            event!(
                Level::DEBUG,
                "Creating secure gRPC connection to {}",
                uri.addr()
            );

            self = self.set_credentials(creds);
            self.connect(&uri.addr())
        } else {
            event!(
                Level::WARN,
                "Creating insecure gRPC connection to {}",
                uri.addr(),
            );

            self.connect(&uri.addr())
        }
    }
}
