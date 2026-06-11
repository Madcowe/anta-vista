use av_probe::tests::trust::test_unknown_trust;
use av_probe::cli::{Cli, Role, OutputFormat};
use av_net_x0x::dispatcher::MessageDispatcher;
use av_net_x0x::mock::MockNetClient;
use std::sync::Arc;
use av_net_x0x::client::X0xConfig;

#[test]
fn test_local_trust_ranking_offline() {
    let conn = av_store::open_in_memory().unwrap();
    let peer_id = "8a3f8902c67de1234567890abcdef1234567890abcdef1234567890abcde";
    
    let args = Cli {
        role: Role::Probe,
        peer: Some(peer_id.to_string()),
        wait: 5,
        test: None,
        output: OutputFormat::Text,
        real_model: false,
        verbose: false,
    };
    
    let mock_client = Arc::new(MockNetClient::new("my-agent"));
    let dispatcher = MessageDispatcher::new(mock_client);
    let (_gossip_tx, gossip_rx) = std::sync::mpsc::channel();
    let (_direct_tx, direct_rx) = std::sync::mpsc::channel();
    let hub = av_probe::tests::helpers::MessageHub {
        gossip_rx,
        direct_rx,
    };
    
    let x0x_cfg = X0xConfig {
        api_base: "http://127.0.0.1:12345".to_string(),
        token: "token".to_string(),
        agent_id: "my-agent".to_string(),
    };
    
    let result = test_unknown_trust(&args, &dispatcher, &hub, &conn, &x0x_cfg);
    assert_eq!(result.status, av_probe::output::TestStatus::Pass);
    println!("Offline trust verification result: {:?}", result);
}
