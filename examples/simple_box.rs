//! A simple demo to showcase how player could send inputs to move a box and server replicates position back.
//! Also demonstrates the single-player and how sever also could be a player.

use bevy::log::{Level, LogPlugin};
use bevy::{
    color::palettes::css::GREEN,
    prelude::*,
    winit::{UpdateMode::Continuous, WinitSettings},
};
use bevy_matchbox::matchbox_signaling::SignalingServer;
use bevy_replicon::prelude::*;
use bevy_replicon_matchbox::{MatchboxClient, MatchboxHost, RepliconMatchboxPlugins};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::net::{Ipv4Addr, SocketAddrV4};

fn main() {
    let log_plugin = LogPlugin {
        level: Level::INFO,
        filter: "bevy_replicon_matchbox=debug,wgpu=error,bevy_matchbox=error,webrtc_ice=error"
            .into(),
        ..default()
    };
    let mut app = App::new();
    app.init_resource::<Cli>() // Parse CLI before creating window.
        // Makes the server/client update continuously even while unfocused.
        .insert_resource(WinitSettings {
            focused_mode: Continuous,
            unfocused_mode: Continuous,
        })
        .add_plugins((
            DefaultPlugins.set(log_plugin),
            RepliconPlugins,
            RepliconMatchboxPlugins,
        ))
        .replicate::<BoxPosition>()
        .replicate::<PlayerBox>()
        .add_client_event::<MoveBox>(Channel::Ordered)
        .add_observer(spawn_clients)
        .add_observer(despawn_clients)
        .add_observer(apply_movement)
        .add_systems(Startup, (read_cli, spawn_camera))
        .add_systems(Update, (read_input, draw_boxes));

    app.run();
}

fn read_cli(mut commands: Commands, cli: Res<Cli>, channels: Res<RepliconChannels>) -> Result<()> {
    match *cli {
        Cli::SinglePlayer => {
            info!("starting single-player game");
            commands.spawn((
                PlayerBox {
                    color: GREEN.into(),
                },
                BoxOwner(ClientId::Server),
            ));
        }
        Cli::Server { port } => {
            info!("starting server at port {port}");
            start_signaling_server(&mut commands, port);
            let room_url = format!("ws://localhost:{port}/simple-box");

            let server = MatchboxHost::new(room_url, &channels)?;
            commands.insert_resource(server);
            commands.spawn((
                Text::new("Server"),
                TextFont {
                    font_size: 30.0,
                    ..Default::default()
                },
                TextColor::WHITE,
            ));
            commands.spawn((
                PlayerBox {
                    color: GREEN.into(),
                },
                BoxOwner(ClientId::Server),
            ));
        }
        Cli::Client { port } => {
            info!("connecting to port {port}");
            let room_url = format!("ws://localhost:{port}/simple-box");

            let client = MatchboxClient::new(room_url, &channels)?;
            commands.insert_resource(client);
            commands.spawn((
                Text::new("Client"),
                TextFont {
                    font_size: 30.0,
                    ..default()
                },
                TextColor::WHITE,
            ));
        }
    }

    Ok(())
}

fn start_signaling_server(commands: &mut Commands, port: u16) {
    info!("Starting signaling server on port {port}");
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
    let signaling_server = bevy_matchbox::MatchboxServer::from(
        SignalingServer::client_server_builder(addr)
            .on_connection_request(|connection| {
                info!("Connecting: {connection:?}");
                Ok(true) // Allow all connections
            })
            .on_id_assignment(|(socket, id)| info!("{socket} received {id}"))
            .on_host_connected(|id| info!("Host joined: {id}"))
            .on_host_disconnected(|id| info!("Host left: {id}"))
            .on_client_connected(|id| info!("Client joined: {id}"))
            .on_client_disconnected(|id| info!("Client left: {id}"))
            .cors()
            .trace()
            .build(),
    );
    commands.insert_resource(signaling_server);
}

fn spawn_camera(mut commands: Commands) {
    commands.spawn(Camera2d);
}

/// Spawns a new box whenever a client connects.
fn spawn_clients(add: On<Add, ConnectedClient>, mut commands: Commands) {
    // Hash index to generate visually distinctive color.
    let mut hasher = DefaultHasher::new();
    add.entity.index().hash(&mut hasher);
    let hash = hasher.finish();

    // Use the lower 24 bits.
    // Divide by 255 to convert bytes into 0..1 floats.
    let r = ((hash >> 16) & 0xFF) as f32 / 255.0;
    let g = ((hash >> 8) & 0xFF) as f32 / 255.0;
    let b = (hash & 0xFF) as f32 / 255.0;

    // Generate pseudo random color from client entity.
    info!("spawning box for `{}`", add.entity);
    commands.spawn((
        PlayerBox {
            color: Color::srgb(r, g, b),
        },
        BoxOwner(ClientId::Client(add.entity)),
    ));
}

/// Despawns a box whenever a client disconnects.
fn despawn_clients(
    remove: On<Remove, ConnectedClient>,
    mut commands: Commands,
    boxes: Query<(Entity, &BoxOwner)>,
) {
    let (entity, _) = boxes
        .iter()
        .find(|&(_, owner)| **owner == ClientId::Client(remove.entity))
        .expect("all clients should have entities");
    commands.entity(entity).despawn();
}

/// Reads player inputs and sends [`MoveBox`] events.
fn read_input(mut commands: Commands, input: Res<ButtonInput<KeyCode>>) {
    let mut direction = Vec2::ZERO;
    if input.pressed(KeyCode::KeyW) {
        direction.y += 1.0;
    }
    if input.pressed(KeyCode::KeyA) {
        direction.x -= 1.0;
    }
    if input.pressed(KeyCode::KeyS) {
        direction.y -= 1.0;
    }
    if input.pressed(KeyCode::KeyD) {
        direction.x += 1.0;
    }

    if direction != Vec2::ZERO {
        commands.client_trigger(MoveBox(direction.normalize_or_zero()));
    }
}

/// Mutates [`BoxPosition`] based on [`MoveBox`] events.
///
/// Fast-paced games usually you don't want to wait until server send a position back because of the latency.
/// But this example just demonstrates simple replication concept.
fn apply_movement(
    move_box: On<FromClient<MoveBox>>,
    time: Res<Time>,
    mut boxes: Query<(&BoxOwner, &mut BoxPosition)>,
) {
    const MOVE_SPEED: f32 = 300.0;

    // Find the sender entity. We don't include the entity as a trigger target to save traffic, since the server knows
    // which entity to apply the input to. We could have a resource that maps connected clients to controlled entities,
    // but we didn't implement it for the sake of simplicity.
    let (_, mut position) = boxes
        .iter_mut()
        .find(|(owner, _)| ***owner == move_box.client_id)
        .unwrap_or_else(|| panic!("`{:?}` should be connected", move_box.client_id));

    **position += *move_box.message * time.delta_secs() * MOVE_SPEED;
}

fn draw_boxes(mut gizmos: Gizmos, boxes: Query<(&BoxPosition, &PlayerBox)>) {
    for (position, player) in &boxes {
        gizmos.rect(
            Vec3::new(position.x, position.y, 0.0),
            Vec2::ONE * 50.0,
            player.color,
        );
    }
}

const PORT: u16 = 5000;

/// A simple game with moving boxes.
#[derive(Parser, PartialEq, Resource)]
enum Cli {
    /// No networking will be used, the player will control its box locally.
    SinglePlayer,
    /// Run game instance will act as both a player and a host.
    Server {
        #[arg(short, long, default_value_t = PORT)]
        port: u16,
    },
    /// The game instance will connect to a host in order to start the game.
    Client {
        #[arg(short, long, default_value_t = PORT)]
        port: u16,
    },
}

impl Default for Cli {
    fn default() -> Self {
        Self::parse()
    }
}

/// Player-controlled box.
///
/// We want to replicate all boxes, so we just set [`Replicated`] as a required component.
#[derive(Component, Deref, Deserialize, Serialize, Default)]
#[require(BoxPosition, Replicated)]
struct PlayerBox {
    /// Color to visually distinguish boxes.
    color: Color,
}

/// Position of a player-controlled box.
///
/// This is a separate component from [`PlayerBox`] because, when the position
/// changes, we only want to send this component (and it changes often!).
#[derive(Component, Deserialize, Serialize, Deref, DerefMut, Default)]
struct BoxPosition(Vec2);

/// Identifies which player controls the box.
///
/// Points to the controlling [`ClientId`]. Used to apply movement to the
/// correct box.
///
/// It's not replicated and present only on server or singleplayer.
#[derive(Component, Clone, Copy, Deref)]
struct BoxOwner(ClientId);

/// A movement event for the controlled box.
#[derive(Deserialize, Deref, Event, Serialize)]
struct MoveBox(Vec2);
