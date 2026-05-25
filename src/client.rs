use crate::shared::*;
use bevy::prelude::*;
use bevy_matchbox::MatchboxSocket;
use bevy_matchbox::matchbox_socket::PeerId;
use bevy_matchbox::prelude::PeerState;
use bevy_replicon::prelude::*;
use std::io;

/// Adds a client messaging backend made for examples to `bevy_replicon`.
pub struct RepliconMatchboxClientPlugin;

impl Plugin for RepliconMatchboxClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PreUpdate,
            (
                receive_packets.run_if(resource_exists::<MatchboxClient>),
                receive_system_channel_packets.run_if(resource_exists::<MatchboxClient>),
                update_peers.run_if(resource_exists::<MatchboxClient>),
            )
                .chain()
                .in_set(ClientSystems::ReceivePackets),
        );

        // Must run in PreUpdate (always), not Send - mirrors the server side
        // so dropping the client transitions `ClientState` reliably.
        app.add_systems(
            PreUpdate,
            set_disconnected
                .in_set(ClientSystems::ReceivePackets)
                .run_if(resource_removed::<MatchboxClient>),
        );
        app.add_systems(
            PostUpdate,
            send_packets
                .in_set(ClientSystems::SendPackets)
                .run_if(not(no_host_defined).and(resource_exists::<MatchboxClient>)),
        );
    }
}

fn no_host_defined(client: Option<Res<MatchboxClient>>) -> bool {
    if let Some(client) = client {
        return client.host_peer_id.is_none();
    }
    true
}

fn set_disconnected(mut client_state: ResMut<NextState<ClientState>>) {
    client_state.set(ClientState::Disconnected);
}

fn update_peers(mut client: ResMut<MatchboxClient>, mut commands: Commands) {
    let Ok(peers) = client.socket.try_update_peers() else {
        commands.remove_resource::<MatchboxClient>();
        return;
    };

    let Some(host_peer_id) = client.host_peer_id else {
        return;
    };
    for (peer_id, state) in peers {
        if matches!(state, PeerState::Disconnected) && peer_id != host_peer_id {
            trace!("host {} disconnected", peer_id);
            commands.remove_resource::<MatchboxClient>();
            return;
        }
    }
}

fn receive_system_channel_packets(
    mut client: ResMut<MatchboxClient>,
    mut client_state: ResMut<NextState<ClientState>>,
) {
    if client.socket.all_channels_closed() {
        trace!("matchbox socket was closed");
        return;
    }
    let Ok(channel) = client.socket.get_channel_mut(SYSTEM_CHANNEL_ID) else {
        error!("system channel not found!");
        return;
    };
    for (peer_id, packet) in channel.receive() {
        let Ok(message) = from_packet(&packet) else {
            error!("failed to deserialize system message {}", packet.len());
            continue;
        };
        trace!(
            "client received system message {:?} from peer {}",
            message, peer_id
        );

        match message {
            SystemChannelMessage::ConnectedToHost => {
                client.host_peer_id = Some(peer_id);
                client_state.set(ClientState::Connected);
            }
            SystemChannelMessage::HostRequestsDisconnect => {
                info!("disconnected by server");
                client.should_disconnect = true;
            }

            SystemChannelMessage::ClientDisconnects => {
                error!("Unexpected message received from host");
            }
        }
    }
}

fn receive_packets(
    mut client: ResMut<MatchboxClient>,
    mut messages: ResMut<ClientMessages>,
    channels: Res<RepliconChannels>,
) {
    if client.socket.all_channels_closed() {
        trace!("matchbox socket was closed");
        return;
    }

    for (channel_id, _) in channels.server_channels().iter().enumerate() {
        //server socket channels are the same as the channel id +1 for the system channel
        let socket_channel_id = 1 + channel_id;
        let Ok(channel) = client.socket.get_channel_mut(socket_channel_id) else {
            continue;
        };
        for (id, packet) in channel.receive() {
            trace!(
                "client received packet from peer {}, c:{} size {}",
                id,
                channel_id,
                packet.len()
            );
            messages.insert_received(channel_id, strip_marker(packet.as_ref()));
        }
    }
}

fn send_packets(
    mut client: ResMut<MatchboxClient>,
    mut messages: ResMut<ClientMessages>,
    mut client_state: ResMut<NextState<ClientState>>,
    channels: Res<RepliconChannels>,
) {
    if client.socket.any_channel_closed() {
        trace!("matchbox socket was closed");
        return;
    }

    let Some(host_peer_id) = client.host_peer_id else {
        error!("set connected before host was defined");
        return;
    };
    for (channel_id, message) in messages.drain_sent() {
        //client socket channels are offset by the server channel length + 1 for the system channel
        let socket_channel_id = 1 + channels.server_channels().len() + channel_id;
        client
            .socket
            .channel_mut(socket_channel_id)
            .send(add_marker(message.as_ref()), host_peer_id);
    }

    if client.should_disconnect {
        client.socket.close();
        client.host_peer_id = None;
        client.should_disconnect = false;
        client_state.set(ClientState::Disconnected);
    }
}

#[derive(Resource)]
pub struct MatchboxClient {
    pub socket: MatchboxSocket,
    pub host_peer_id: Option<PeerId>,
    should_disconnect: bool,
}

impl MatchboxClient {
    pub fn new(
        room_url: impl Into<String>,
        replicon_channels: &RepliconChannels,
    ) -> io::Result<Self> {
        let socket = create_matchbox_socket(room_url, replicon_channels);
        Ok(Self {
            socket,
            host_peer_id: None,
            should_disconnect: false,
        })
    }

    pub fn is_connected(&self) -> bool {
        self.host_peer_id.is_some()
    }

    pub fn disconnect(&mut self) {
        let Ok(channel) = self.socket.get_channel_mut(SYSTEM_CHANNEL_ID) else {
            return;
        };
        let Some(host_peer) = self.host_peer_id else {
            return;
        };
        trace!("sending disconnect message to host");
        let mut buf = [0u8; 1];
        let package = to_packet(&SystemChannelMessage::ClientDisconnects, &mut buf).into();
        channel.send(package, host_peer);
        self.should_disconnect = true;
    }
}
