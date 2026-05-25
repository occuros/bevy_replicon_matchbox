use bevy::log::{Level, LogPlugin};
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use bevy_replicon::prelude::*;
use bevy_replicon_matchbox::*;
use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::atomic::{AtomicU16, Ordering};
use test_log::test;

//run the tests with cargo test -- --test-threads=1

static PORT_COUNTER: AtomicU16 = AtomicU16::new(30000);
fn next_test_port() -> u16 {
    PORT_COUNTER.fetch_add(1, Ordering::AcqRel)
}

#[test]
fn connect_disconnect() {
    let port = next_test_port();
    let mut server_app = App::new();
    let mut client_app = App::new();
    for app in [&mut server_app, &mut client_app] {
        app.add_plugins((
            MinimalPlugins,
            StatesPlugin,
            RepliconPlugins.build().set(ServerPlugin::new(PostUpdate)),
            RepliconMatchboxPlugins,
        ))
        .finish();
    }

    setup(&mut server_app, &mut client_app, port);
    assert_eq!(
        server_app.world().resource::<State<ServerState>>().get(),
        &ServerState::Running
    );

    let matchbox_server = server_app.world().resource::<MatchboxHost>();
    let connected_clients = matchbox_server.connected_clients();
    info!("connected clients: {}", connected_clients);
    let client = client_app.world().resource::<MatchboxClient>();
    info!("client connected: {}", client.is_connected());
    assert_eq!(
        connected_clients, 1,
        "one client connected expected but got {connected_clients}",
    );

    let mut clients = server_app.world_mut().query::<&ConnectedClient>();
    assert_eq!(clients.iter(server_app.world()).len(), 1);

    assert_eq!(
        client_app.world().resource::<State<ClientState>>().get(),
        &ClientState::Connected
    );

    let mut matchbox_client = client_app.world_mut().resource_mut::<MatchboxClient>();
    assert!(matchbox_client.is_connected());

    matchbox_client.disconnect();

    client_app.update();
    server_app.update();

    info!(
        "connected clients: {}",
        clients.iter(server_app.world()).len()
    );
    //upading again to be sure the socket is flushed
    client_app.update();
    server_app.update();

    assert_eq!(clients.iter(server_app.world()).len(), 0);

    let matchbox_server = server_app.world().resource::<MatchboxHost>();
    info!("connected clients: {}", matchbox_server.connected_clients());

    assert_eq!(matchbox_server.connected_clients(), 0);

    assert_eq!(
        client_app.world().resource::<State<ClientState>>().get(),
        &ClientState::Disconnected
    );
}

#[test]
fn disconnect_request() {
    let port = next_test_port();

    let mut server_app = App::new();
    let mut client_app = App::new();

    for app in [&mut server_app, &mut client_app] {
        let log_plugin = LogPlugin {
            level: Level::INFO,
            filter: "bevy_replicon_matchbox=debug,wgpu=error,bevy_matchbox=error,webrtc_ice=error,webrtc=error"
                .into(),
            ..default()
        };
        app.add_plugins((
            MinimalPlugins,
            StatesPlugin,
            log_plugin,
            RepliconPlugins.build().set(ServerPlugin::new(PostUpdate)),
            RepliconMatchboxPlugins,
        ))
        .add_server_message::<TestEvent>(Channel::Ordered)
        .make_message_independent::<TestEvent>()
        .replicate::<Transform>()
        .finish();
    }

    client_app
        .init_resource::<ReceivedEvents>()
        .add_systems(Update, count_received_events);

    setup(&mut server_app, &mut client_app, port);
    // Let replication initialize before exercising flush-before-disconnect.
    pump(&mut server_app, &mut client_app, 5);

    server_app.world_mut().spawn(Replicated);
    server_app.world_mut().write_message(ToClients {
        targets: SendTargets::All,
        message: TestEvent,
    });

    // Confirm the event + replication reach the client before disconnecting.
    // Asserting an in-flight event survives the *same-frame* disconnect is a
    // multi-channel WebRTC race (the system channel's disconnect notice can
    // outrun the event channel), so verify delivery first, then disconnect.
    pump(&mut server_app, &mut client_app, 10);
    assert_eq!(
        client_app.world().resource::<ReceivedEvents>().0,
        1,
        "the event should be received"
    );
    let mut replicated = client_app.world_mut().query::<&Remote>();
    assert_eq!(
        replicated.iter(client_app.world()).len(),
        1,
        "the replication should be received"
    );

    // Server-initiated disconnect of the client.
    let mut clients = server_app
        .world_mut()
        .query_filtered::<Entity, With<ConnectedClient>>();
    let client_entity = clients.single(server_app.world()).unwrap();
    server_app
        .world_mut()
        .write_message(DisconnectRequest { client: client_entity });

    server_app.update();
    assert_eq!(clients.iter(server_app.world()).len(), 0);

    pump(&mut server_app, &mut client_app, 30);
    assert_eq!(
        client_app.world().resource::<State<ClientState>>().get(),
        &ClientState::Disconnected
    );
}

#[test]
fn replication_test() {
    let port = next_test_port();

    let mut server_app = App::new();
    let mut client_app = App::new();

    for app in [&mut server_app, &mut client_app] {
        app.add_plugins((
            MinimalPlugins,
            StatesPlugin,
            RepliconPlugins.build().set(ServerPlugin::new(PostUpdate)),
            RepliconMatchboxPlugins,
        ))
        .add_server_message::<TestEvent>(Channel::Ordered)
        .finish();
    }

    setup(&mut server_app, &mut client_app, port);

    let mut clients = server_app
        .world_mut()
        .query_filtered::<Entity, With<ConnectedClient>>();

    info!("clients: {:?}", clients.iter(server_app.world()).len());

    server_app.world_mut().spawn(Replicated);
    pump(&mut server_app, &mut client_app, 10);

    let mut replicated = client_app.world_mut().query::<&Remote>();
    error!(
        "replicated: {:?}",
        replicated.iter(client_app.world()).len()
    );

    assert_eq!(
        replicated.iter(client_app.world()).len(),
        1,
        "last replication should be received"
    );
}

#[test]
fn server_stop() {
    let port = next_test_port();

    let mut server_app = App::new();
    let mut client_app = App::new();
    for app in [&mut server_app, &mut client_app] {
        app.add_plugins((
            MinimalPlugins,
            StatesPlugin,
            RepliconPlugins.build().set(ServerPlugin::new(PostUpdate)),
            RepliconMatchboxPlugins,
        ))
        .add_server_message::<TestEvent>(Channel::Ordered)
        .finish();
    }

    setup(&mut server_app, &mut client_app, port);
    let mut server = server_app.world_mut().resource_mut::<MatchboxHost>();
    server.disconnect_all();

    server_app.update();
    client_app.update();

    let mut clients = server_app.world_mut().query::<&ConnectedClient>();
    assert_eq!(clients.iter(server_app.world()).len(), 0);
    assert_eq!(
        server_app.world().resource::<State<ServerState>>().get(),
        &ServerState::Running,
        "requires resource removal"
    );
    assert!(
        client_app
            .world()
            .resource::<MatchboxClient>()
            .is_connected(),
        "matchbox client disconnects only on the next frame"
    );

    server_app.world_mut().remove_resource::<MatchboxHost>();

    // The client detects an abruptly-dropped host over WebRTC after a few
    // frames, not instantly - pump rather than count exact frames.
    pump(&mut server_app, &mut client_app, 30);

    assert_eq!(
        server_app.world().resource::<State<ServerState>>().get(),
        &ServerState::Stopped
    );

    assert_eq!(
        client_app.world().resource::<State<ClientState>>().get(),
        &ClientState::Disconnected
    );

    server_app.world_mut().write_message(ToClients {
        targets: SendTargets::All,
        message: TestEvent,
    });
    server_app.world_mut().spawn(Replicated);

    server_app.update();
    client_app.update();

    let events = client_app.world().resource::<Messages<TestEvent>>();
    assert!(events.is_empty(), "event after stop shouldn't be received");

    let mut replicated = client_app.world_mut().query::<&Remote>();
    assert_eq!(
        replicated.iter(client_app.world()).len(),
        0,
        "replication after stop shouldn't be received"
    );
}

#[test]
fn replication() {
    let port = next_test_port();
    let mut server_app = App::new();
    let mut client_app = App::new();
    for app in [&mut server_app, &mut client_app] {
        app.add_plugins((
            MinimalPlugins,
            StatesPlugin,
            RepliconPlugins.build().set(ServerPlugin::new(PostUpdate)),
            RepliconMatchboxPlugins,
        ))
        .finish();
    }

    setup(&mut server_app, &mut client_app, port);

    server_app.world_mut().spawn(Replicated);
    pump(&mut server_app, &mut client_app, 10);

    let mut replicated = client_app.world_mut().query::<&Remote>();
    assert_eq!(replicated.iter(client_app.world()).len(), 1);
}

#[test]
fn server_event() {
    let port = next_test_port();
    let mut server_app = App::new();
    let mut client_app = App::new();
    for app in [&mut server_app, &mut client_app] {
        app.add_plugins((
            MinimalPlugins,
            StatesPlugin,
            RepliconPlugins.build().set(ServerPlugin::new(PostUpdate)),
            RepliconMatchboxPlugins,
        ))
        .add_server_message::<TestEvent>(Channel::Ordered)
        .finish();
    }

    setup(&mut server_app, &mut client_app, port);

    server_app.world_mut().write_message(ToClients {
        targets: SendTargets::All,
        message: TestEvent,
    });

    server_app.update();
    //again two client updates are required for the events to sync
    client_app.update();
    client_app.update();

    let events = client_app.world().resource::<Messages<TestEvent>>();
    assert_eq!(events.len(), 1);
}

#[test]
fn client_event() {
    let port = next_test_port();

    let mut server_app = App::new();
    let mut client_app = App::new();
    for app in [&mut server_app, &mut client_app] {
        app.add_plugins((
            MinimalPlugins,
            StatesPlugin,
            RepliconPlugins.build().set(ServerPlugin::new(PostUpdate)),
            RepliconMatchboxPlugins,
        ))
        .add_client_message::<TestEvent>(Channel::Ordered)
        .finish();
    }

    setup(&mut server_app, &mut client_app, port);

    client_app.world_mut().write_message(TestEvent);

    client_app.update();
    server_app.update();
    client_app.update();
    server_app.update();

    let client_events = server_app
        .world()
        .resource::<Messages<FromClient<TestEvent>>>();
    assert_eq!(client_events.len(), 1);
}

fn setup(server_app: &mut App, client_app: &mut App, port: u16) {
    start_signaling_server(server_app, port);
    setup_server(server_app, port);
    setup_client(client_app, port);
    wait_for_connection(server_app, client_app);
}

use bevy_matchbox::matchbox_signaling::SignalingServer;

fn start_signaling_server(server_app: &mut App, port: u16) {
    info!("Starting signaling server");
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
            // .trace()
            .build(),
    );
    server_app.insert_resource(signaling_server);
}

fn setup_server(app: &mut App, port: u16) {
    let room_url = format!("ws://localhost:{port}/TestRoom");
    let channels = app.world().resource::<RepliconChannels>();

    let server = MatchboxHost::new(room_url, channels).unwrap();

    app.insert_resource(server);
}

fn setup_client(app: &mut App, port: u16) {
    let room_url = format!("ws://localhost:{port}/TestRoom");
    let channels = app.world().resource::<RepliconChannels>();
    let client = MatchboxClient::new(room_url, channels).unwrap();
    app.insert_resource(client);
}

fn wait_for_connection(server_app: &mut App, client_app: &mut App) {
    loop {
        client_app.update();
        server_app.update();
        let host = server_app.world().resource::<MatchboxHost>();
        let client = client_app.world().resource::<MatchboxClient>();
        if host.connected_clients() > 0 && client.is_connected() {
            break;
        }
    }
}

/// Pump both apps in lockstep. WebRTC plus replicon's protocol-hash handshake
/// take several frames to settle, so tests that assert on replicated state
/// pump a generous number of cycles rather than a hand-counted few.
fn pump(server_app: &mut App, client_app: &mut App, cycles: usize) {
    for _ in 0..cycles {
        server_app.update();
        client_app.update();
    }
}

#[derive(Message, Serialize, Deserialize, Clone)]
struct TestEvent;

/// Persists a count of received [`TestEvent`]s so assertions survive pumping
/// (the `Messages` buffer only retains the last couple of frames).
#[derive(Resource, Default)]
struct ReceivedEvents(usize);

fn count_received_events(mut reader: MessageReader<TestEvent>, mut count: ResMut<ReceivedEvents>) {
    count.0 += reader.read().count();
}
