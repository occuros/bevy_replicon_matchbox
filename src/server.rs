use crate::shared::*;
use bevy::prelude::*;
use bevy_matchbox::MatchboxSocket;
use bevy_matchbox::matchbox_socket::Packet;
use bevy_matchbox::prelude::{PeerId, PeerState};
use bevy_replicon::prelude::*;
use bevy_replicon::shared::backend::connected_client::NetworkId;
use std::collections::HashMap;
use std::io;

pub struct RepliconMatchboxServerPlugin;

impl Plugin for RepliconMatchboxServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PreUpdate,
            (
                set_running.run_if(resource_added::<MatchboxHost>),
                receive_system_channel_packets.run_if(resource_exists::<MatchboxHost>),
                receive_packets.run_if(resource_exists::<MatchboxHost>),
                received_disconnect.run_if(resource_exists::<MatchboxHost>),
            )
                .chain()
                .in_set(ServerSystems::ReceivePackets),
        );
        // Must run in PreUpdate (always), not Send (only on tick change) -
        // otherwise stopping the server never transitions `ServerState`.
        app.add_systems(
            PreUpdate,
            set_stopped
                .in_set(ServerSystems::ReceivePackets)
                .run_if(resource_removed::<MatchboxHost>),
        );
        app.add_systems(
            PostUpdate,
            (
                update_client_presence
                    .in_set(ServerSystems::SendPackets)
                    .run_if(resource_exists::<MatchboxHost>),
                send_packets
                    .in_set(ServerSystems::SendPackets)
                    .run_if(resource_exists::<MatchboxHost>)
                    .after(update_client_presence)
                    .before(received_disconnect),
            ),
        );
    }
}

fn set_stopped(mut server_state: ResMut<NextState<ServerState>>) {
    trace!("server stopped");
    server_state.set(ServerState::Stopped);
}

fn set_running(mut server_state: ResMut<NextState<ServerState>>) {
    server_state.set(ServerState::Running);
}

fn update_client_presence(mut commands: Commands, mut server: ResMut<MatchboxHost>) {
    let Ok(updated_peers) = server.socket.try_update_peers() else {
        for client_entity in server.client_entities.values() {
            commands.entity(*client_entity).despawn();
        }
        error!("sockets closed, shutting down");
        commands.remove_resource::<MatchboxHost>();
        return;
    };

    for (peer, state) in updated_peers {
        match state {
            PeerState::Connected => {
                if server.client_entities.contains_key(&peer) {
                    continue;
                }
                let network_id = NetworkId::new(uuid_to_u64_truncated(peer));
                let client_entity = commands
                    .spawn((
                        ConnectedClient { max_size: 1200 },
                        network_id,
                        MatchboxClientConnection { peer_id: peer },
                    ))
                    .id();
                trace!(
                    "new client peer: {}, network_id: {:?} entity: {}",
                    peer, network_id, client_entity
                );
                server.client_entities.insert(peer, client_entity);
                let mut buf = [0u8; 1];
                let packet: Packet =
                    to_packet(&SystemChannelMessage::ConnectedToHost, &mut buf).into();
                server
                    .socket
                    .channel_mut(SYSTEM_CHANNEL_ID)
                    .send(packet, peer);
            }
            PeerState::Disconnected => {
                let Some(client_entity) = server.client_entities.remove(&peer) else {
                    continue;
                };
                trace!("client disconnected {:?}: {}", peer, client_entity);
                commands.entity(client_entity).despawn();
            }
        }
    }
}

fn receive_system_channel_packets(mut commands: Commands, mut server: ResMut<MatchboxHost>) {
    if server.socket.all_channels_closed() {
        trace!("matchbox socket was closed");
        return;
    }
    let Ok(channel) = server.socket.get_channel_mut(SYSTEM_CHANNEL_ID) else {
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
            SystemChannelMessage::ClientDisconnects => {
                let Some(client_entity) = server.client_entities.remove(&peer_id) else {
                    continue;
                };
                trace!("client disconnected {peer_id}: {client_entity}");
                commands.entity(client_entity).despawn();
            }
            _ => {
                error!("Unexpected message {message:?} received from client {peer_id}");
            }
        }
    }
}

fn receive_packets(
    mut messages: ResMut<ServerMessages>,
    mut server: ResMut<MatchboxHost>,
    channels: Res<RepliconChannels>,
) {
    for (channel_id, _) in channels.client_channels().iter().enumerate() {
        let socket_channel_id = 1 + channels.server_channels().len() + channel_id;
        for (id, packet) in server.socket.channel_mut(socket_channel_id).receive() {
            let Some(client_entity) = server.client_entities.get(&id) else {
                trace!("received packet from unknown client {}", id);
                continue;
            };
            messages.insert_received(*client_entity, channel_id, strip_marker(&packet));
        }
    }
}

fn send_packets(
    mut commands: Commands,
    mut messages: ResMut<ServerMessages>,
    mut server: ResMut<MatchboxHost>,
    clients: Query<&MatchboxClientConnection>,
) {
    for (client_entity, channel_id, message) in messages.drain_sent() {
        let Ok(connection) = clients.get(client_entity) else {
            trace!("client {} not connected", client_entity);
            continue;
        };
        if !server.client_entities.contains_key(&connection.peer_id) {
            trace!("client {} was disconnected", client_entity);
            continue;
        }
        trace!(
            "sending packet to client {}: c:{} - {:?}",
            client_entity,
            channel_id,
            add_marker(message.as_ref()).len()
        );
        let socket_channel_id = 1 + channel_id;
        server
            .socket
            .channel_mut(socket_channel_id)
            .send(add_marker(message.as_ref()), connection.peer_id);
    }
    let disconnect_ids: Vec<_> = server.clients_to_disconnect.drain(..).collect();

    for peer_id in disconnect_ids {
        let Some(client_entity) = server.client_entities.remove(&peer_id) else {
            continue;
        };
        let mut buf = [0u8; 1];
        let packet: Packet =
            to_packet(&SystemChannelMessage::HostRequestsDisconnect, &mut buf).into();
        server
            .socket
            .channel_mut(SYSTEM_CHANNEL_ID)
            .send(packet, peer_id);
        trace!("disconnecting client `{}`", client_entity);
        commands.entity(client_entity).despawn();
    }
}

fn received_disconnect(
    mut disconnect_events: MessageReader<DisconnectRequest>,
    mut server: ResMut<MatchboxHost>,
    client_connections: Query<&MatchboxClientConnection>,
) {
    for event in disconnect_events.read() {
        let Ok(connection) = client_connections.get(event.client) else {
            continue;
        };
        trace!(
            "queuing disconnecting client `{}` by request",
            event.client
        );
        server.clients_to_disconnect.push(connection.peer_id);
    }
}

// The socket used by the server.
#[derive(Resource)]
pub struct MatchboxHost {
    pub socket: MatchboxSocket,
    pub client_entities: HashMap<PeerId, Entity>,
    pub clients_to_disconnect: Vec<PeerId>,
}

impl MatchboxHost {
    pub fn new(
        room_url: impl Into<String>,
        replicon_channels: &RepliconChannels,
    ) -> io::Result<Self> {
        let socket = create_matchbox_socket(room_url, replicon_channels);

        Ok(Self {
            socket,
            // unreliable_socket,
            client_entities: HashMap::new(),
            clients_to_disconnect: Vec::new(),
        })
    }

    pub fn connected_clients(&self) -> usize {
        self.client_entities.len()
    }

    pub fn disconnect_all(&mut self) {
        self.clients_to_disconnect
            .extend(self.client_entities.keys().cloned());
    }
}

#[derive(Component)]
struct MatchboxClientConnection {
    pub peer_id: PeerId,
}
