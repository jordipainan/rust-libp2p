// Copyright 2018 Parity Technologies (UK) Ltd.
//
// Permission is hereby granted, free of charge, to any person obtaining a
// copy of this software and associated documentation files (the "Software"),
// to deal in the Software without restriction, including without limitation
// the rights to use, copy, modify, merge, publish, distribute, sublicense,
// and/or sell copies of the Software, and to permit persons to whom the
// Software is furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
// OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
// FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

//! High level manager of the network.
//!
//! The `Swarm` struct contains the state of the network as a whole. The entire behaviour of a
//! libp2p network can be controlled through the `Swarm`.
//!
//! # Initializing a Swarm
//!
//! Creating a `Swarm` requires three things:
//! 
//! - An implementation of the `Transport` trait. This is the type that will be used in order to
//!   reach nodes on the network based on their address. See the `transport` module for more
//!   information.
//! - An implementation of the `NetworkBehaviour` trait. This is a state machine that defines how
//!   the swarm should behave once it is connected to a node.
//! - An implementation of the `Topology` trait. This is a container that holds the list of nodes
//!   that we think are part of the network. See the `topology` module for more information.
//!
//! # Network behaviour
//!
//! The `NetworkBehaviour` trait is implemented on types that indicate to the swarm how it should
//! behave. This includes which protocols are supported and which nodes to try to connect to.
//!

use crate::{
    Transport, Multiaddr, PublicKey, PeerId, InboundUpgrade, OutboundUpgrade, UpgradeInfo, ProtocolName,
    muxing::StreamMuxer,
    nodes::{
        handled_node::NodeHandler,
        node::Substream,
        raw_swarm::{RawSwarm, RawSwarmEvent}
    },
    protocols_handler::{NodeHandlerWrapper, ProtocolsHandler},
    topology::Topology
};
use futures::prelude::*;
use smallvec::SmallVec;
use std::{fmt, io, ops::{Deref, DerefMut}};

pub use crate::nodes::raw_swarm::ConnectedPoint;

/// Contains the state of the network, plus the way it should behave.
pub struct Swarm<TTransport, TBehaviour, TTopology>
where TTransport: Transport,
      TBehaviour: NetworkBehaviour<TTopology>,
{
    raw_swarm: RawSwarm<
        TTransport,
        <<TBehaviour as NetworkBehaviour<TTopology>>::ProtocolsHandler as ProtocolsHandler>::InEvent,
        <<TBehaviour as NetworkBehaviour<TTopology>>::ProtocolsHandler as ProtocolsHandler>::OutEvent,
        NodeHandlerWrapper<TBehaviour::ProtocolsHandler>,
        <<TBehaviour as NetworkBehaviour<TTopology>>::ProtocolsHandler as ProtocolsHandler>::Error,
    >,

    /// Handles which nodes to connect to and how to handle the events sent back by the protocol
    /// handlers.
    behaviour: TBehaviour,

    /// Holds the topology of the network. In other words all the nodes that we think exist, even
    /// if we're not connected to them.
    topology: TTopology,

    /// List of protocols that the behaviour says it supports.
    supported_protocols: SmallVec<[Vec<u8>; 16]>,

    /// List of multiaddresses we're listening on.
    listened_addrs: SmallVec<[Multiaddr; 8]>,
}

impl<TTransport, TBehaviour, TTopology> Deref for Swarm<TTransport, TBehaviour, TTopology>
where TTransport: Transport,
      TBehaviour: NetworkBehaviour<TTopology>,
{
    type Target = TBehaviour;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.behaviour
    }
}

impl<TTransport, TBehaviour, TTopology> DerefMut for Swarm<TTransport, TBehaviour, TTopology>
where TTransport: Transport,
      TBehaviour: NetworkBehaviour<TTopology>,
{
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.behaviour
    }
}

impl<TTransport, TBehaviour, TMuxer, TTopology> Swarm<TTransport, TBehaviour, TTopology>
where TBehaviour: NetworkBehaviour<TTopology>,
      TMuxer: StreamMuxer + Send + Sync + 'static,
      <TMuxer as StreamMuxer>::OutboundSubstream: Send + 'static,
      <TMuxer as StreamMuxer>::Substream: Send + 'static,
      TTransport: Transport<Output = (PeerId, TMuxer)> + Clone,
      TTransport::Listener: Send + 'static,
      TTransport::ListenerUpgrade: Send + 'static,
      TTransport::Dial: Send + 'static,
      TBehaviour::ProtocolsHandler: ProtocolsHandler<Substream = Substream<TMuxer>> + Send + 'static,
      <TBehaviour::ProtocolsHandler as ProtocolsHandler>::InEvent: Send + 'static,
      <TBehaviour::ProtocolsHandler as ProtocolsHandler>::OutEvent: Send + 'static,
      <TBehaviour::ProtocolsHandler as ProtocolsHandler>::Error: Send + 'static,
      <TBehaviour::ProtocolsHandler as ProtocolsHandler>::OutboundOpenInfo: Send + 'static, // TODO: shouldn't be necessary
      <TBehaviour::ProtocolsHandler as ProtocolsHandler>::InboundProtocol: InboundUpgrade<Substream<TMuxer>> + Send + 'static,
      <<TBehaviour::ProtocolsHandler as ProtocolsHandler>::InboundProtocol as UpgradeInfo>::Info: Send + 'static,
      <<TBehaviour::ProtocolsHandler as ProtocolsHandler>::InboundProtocol as UpgradeInfo>::InfoIter: Send + 'static,
      <<<TBehaviour::ProtocolsHandler as ProtocolsHandler>::InboundProtocol as UpgradeInfo>::InfoIter as IntoIterator>::IntoIter: Send + 'static,
      <<TBehaviour::ProtocolsHandler as ProtocolsHandler>::InboundProtocol as InboundUpgrade<Substream<TMuxer>>>::Error: fmt::Debug + Send + 'static,
      <<TBehaviour::ProtocolsHandler as ProtocolsHandler>::InboundProtocol as InboundUpgrade<Substream<TMuxer>>>::Future: Send + 'static,
      <TBehaviour::ProtocolsHandler as ProtocolsHandler>::OutboundProtocol: OutboundUpgrade<Substream<TMuxer>> + Send + 'static,
      <<TBehaviour::ProtocolsHandler as ProtocolsHandler>::OutboundProtocol as UpgradeInfo>::Info: Send + 'static,
      <<TBehaviour::ProtocolsHandler as ProtocolsHandler>::OutboundProtocol as UpgradeInfo>::InfoIter: Send + 'static,
      <<<TBehaviour::ProtocolsHandler as ProtocolsHandler>::OutboundProtocol as UpgradeInfo>::InfoIter as IntoIterator>::IntoIter: Send + 'static,
      <<TBehaviour::ProtocolsHandler as ProtocolsHandler>::OutboundProtocol as OutboundUpgrade<Substream<TMuxer>>>::Future: Send + 'static,
      <<TBehaviour::ProtocolsHandler as ProtocolsHandler>::OutboundProtocol as OutboundUpgrade<Substream<TMuxer>>>::Error: fmt::Debug + Send + 'static,
      <NodeHandlerWrapper<TBehaviour::ProtocolsHandler> as NodeHandler>::OutboundOpenInfo: Send + 'static, // TODO: shouldn't be necessary
      TTopology: Topology,
{
    /// Builds a new `Swarm`.
    #[inline]
    pub fn new(transport: TTransport, mut behaviour: TBehaviour, topology: TTopology) -> Self {
        let supported_protocols = behaviour
            .new_handler()
            .listen_protocol()
            .protocol_info()
            .into_iter()
            .map(|info| info.protocol_name().to_vec())
            .collect();

        let raw_swarm = RawSwarm::new(transport, topology.local_peer_id().clone());

        Swarm {
            raw_swarm,
            behaviour,
            topology,
            supported_protocols,
            listened_addrs: SmallVec::new(),
        }
    }

    /// Returns the transport passed when building this object.
    #[inline]
    pub fn transport(me: &Self) -> &TTransport {
        me.raw_swarm.transport()
    }

    /// Starts listening on the given address.
    ///
    /// Returns an error if the address is not supported.
    /// On success, returns an alternative version of the address.
    #[inline]
    pub fn listen_on(me: &mut Self, addr: Multiaddr) -> Result<Multiaddr, Multiaddr> {
        let result = me.raw_swarm.listen_on(addr);
        if let Ok(ref addr) = result {
            me.listened_addrs.push(addr.clone());
        }
        result
    }

    /// Tries to dial the given address.
    ///
    /// Returns an error if the address is not supported.
    #[inline]
    pub fn dial_addr(me: &mut Self, addr: Multiaddr) -> Result<(), Multiaddr> {
        let handler = me.behaviour.new_handler();
        me.raw_swarm.dial(addr, handler.into_node_handler())
    }

    /// Tries to reach the given peer using the elements in the topology.
    ///
    /// Has no effect if we are already connected to that peer, or if no address is known for the
    /// peer.
    #[inline]
    pub fn dial(me: &mut Self, peer_id: PeerId) {
        let addrs = me.topology.addresses_of_peer(&peer_id);
        let handler = me.behaviour.new_handler().into_node_handler();
        if let Some(peer) = me.raw_swarm.peer(peer_id).as_not_connected() {
            let _ = peer.connect_iter(addrs, handler);
        }
    }

    /// Returns an iterator that produces the list of addresses we're listening on.
    #[inline]
    pub fn listeners(me: &Self) -> impl Iterator<Item = &Multiaddr> {
        RawSwarm::listeners(&me.raw_swarm)
    }

    /// Returns the peer ID of the swarm passed as parameter.
    #[inline]
    pub fn local_peer_id(me: &Self) -> &PeerId {
        &me.raw_swarm.local_peer_id()
    }

    /// Returns the topology of the swarm.
    #[inline]
    pub fn topology(me: &Self) -> &TTopology {
        &me.topology
    }

    /// Returns the topology of the swarm.
    #[inline]
    pub fn topology_mut(me: &mut Self) -> &mut TTopology {
        &mut me.topology
    }
}

impl<TTransport, TBehaviour, TMuxer, TTopology> Stream for Swarm<TTransport, TBehaviour, TTopology>
where TBehaviour: NetworkBehaviour<TTopology>,
      TMuxer: StreamMuxer + Send + Sync + 'static,
      <TMuxer as StreamMuxer>::OutboundSubstream: Send + 'static,
      <TMuxer as StreamMuxer>::Substream: Send + 'static,
      TTransport: Transport<Output = (PeerId, TMuxer)> + Clone,
      TTransport::Listener: Send + 'static,
      TTransport::ListenerUpgrade: Send + 'static,
      TTransport::Dial: Send + 'static,
      TBehaviour::ProtocolsHandler: ProtocolsHandler<Substream = Substream<TMuxer>> + Send + 'static,
      <TBehaviour::ProtocolsHandler as ProtocolsHandler>::InEvent: Send + 'static,
      <TBehaviour::ProtocolsHandler as ProtocolsHandler>::OutEvent: Send + 'static,
      <TBehaviour::ProtocolsHandler as ProtocolsHandler>::Error: Send + 'static,
      <TBehaviour::ProtocolsHandler as ProtocolsHandler>::OutboundOpenInfo: Send + 'static, // TODO: shouldn't be necessary
      <TBehaviour::ProtocolsHandler as ProtocolsHandler>::InboundProtocol: InboundUpgrade<Substream<TMuxer>> + Send + 'static,
      <<TBehaviour::ProtocolsHandler as ProtocolsHandler>::InboundProtocol as InboundUpgrade<Substream<TMuxer>>>::Future: Send + 'static,
      <<TBehaviour::ProtocolsHandler as ProtocolsHandler>::InboundProtocol as InboundUpgrade<Substream<TMuxer>>>::Error: fmt::Debug + Send + 'static,
      <<TBehaviour::ProtocolsHandler as ProtocolsHandler>::InboundProtocol as UpgradeInfo>::Info: Send + 'static,
      <<TBehaviour::ProtocolsHandler as ProtocolsHandler>::InboundProtocol as UpgradeInfo>::InfoIter: Send + 'static,
      <<<TBehaviour::ProtocolsHandler as ProtocolsHandler>::InboundProtocol as UpgradeInfo>::InfoIter as IntoIterator>::IntoIter: Send + 'static,
      <TBehaviour::ProtocolsHandler as ProtocolsHandler>::OutboundProtocol: OutboundUpgrade<Substream<TMuxer>> + Send + 'static,
      <<TBehaviour::ProtocolsHandler as ProtocolsHandler>::OutboundProtocol as OutboundUpgrade<Substream<TMuxer>>>::Future: Send + 'static,
      <<TBehaviour::ProtocolsHandler as ProtocolsHandler>::OutboundProtocol as OutboundUpgrade<Substream<TMuxer>>>::Error: fmt::Debug + Send + 'static,
      <<TBehaviour::ProtocolsHandler as ProtocolsHandler>::OutboundProtocol as UpgradeInfo>::Info: Send + 'static,
      <<TBehaviour::ProtocolsHandler as ProtocolsHandler>::OutboundProtocol as UpgradeInfo>::InfoIter: Send + 'static,
      <<<TBehaviour::ProtocolsHandler as ProtocolsHandler>::OutboundProtocol as UpgradeInfo>::InfoIter as IntoIterator>::IntoIter: Send + 'static,
      <NodeHandlerWrapper<TBehaviour::ProtocolsHandler> as NodeHandler>::OutboundOpenInfo: Send + 'static, // TODO: shouldn't be necessary
      TTopology: Topology,
{
    type Item = TBehaviour::OutEvent;
    type Error = io::Error;

    #[inline]
    fn poll(&mut self) -> Poll<Option<TBehaviour::OutEvent>, io::Error> {
        loop {
            let mut raw_swarm_not_ready = false;

            match self.raw_swarm.poll() {
                Async::NotReady => raw_swarm_not_ready = true,
                Async::Ready(RawSwarmEvent::NodeEvent { peer_id, event }) => {
                    self.behaviour.inject_node_event(peer_id, event);
                },
                Async::Ready(RawSwarmEvent::Connected { peer_id, endpoint }) => {
                    self.behaviour.inject_connected(peer_id, endpoint);
                },
                Async::Ready(RawSwarmEvent::NodeClosed { peer_id, endpoint }) |
                Async::Ready(RawSwarmEvent::NodeError { peer_id, endpoint, .. }) => {
                    self.behaviour.inject_disconnected(&peer_id, endpoint);
                },
                Async::Ready(RawSwarmEvent::Replaced { peer_id, closed_endpoint, endpoint }) => {
                    self.behaviour.inject_disconnected(&peer_id, closed_endpoint);
                    self.behaviour.inject_connected(peer_id, endpoint);
                },
                Async::Ready(RawSwarmEvent::IncomingConnection(incoming)) => {
                    let handler = self.behaviour.new_handler();
                    incoming.accept(handler.into_node_handler());
                },
                Async::Ready(RawSwarmEvent::ListenerClosed { .. }) => {},
                Async::Ready(RawSwarmEvent::IncomingConnectionError { .. }) => {},
                Async::Ready(RawSwarmEvent::DialError { .. }) => {},
                Async::Ready(RawSwarmEvent::UnknownPeerDialError { .. }) => {},
            }

            let behaviour_poll = {
                let transport = self.raw_swarm.transport();
                let mut parameters = PollParameters {
                    topology: &mut self.topology,
                    supported_protocols: &self.supported_protocols,
                    listened_addrs: &self.listened_addrs,
                    nat_traversal: &move |a, b| transport.nat_traversal(a, b),
                };
                self.behaviour.poll(&mut parameters)
            };

            match behaviour_poll {
                Async::NotReady if raw_swarm_not_ready => return Ok(Async::NotReady),
                Async::NotReady => (),
                Async::Ready(NetworkBehaviourAction::GenerateEvent(event)) => {
                    return Ok(Async::Ready(Some(event)));
                },
                Async::Ready(NetworkBehaviourAction::DialAddress { address }) => {
                    let _ = Swarm::dial_addr(self, address);
                },
                Async::Ready(NetworkBehaviourAction::DialPeer { peer_id }) => {
                    Swarm::dial(self, peer_id)
                },
                Async::Ready(NetworkBehaviourAction::SendEvent { peer_id, event }) => {
                    if let Some(mut peer) = self.raw_swarm.peer(peer_id).as_connected() {
                        peer.send_event(event);
                    }
                },
                Async::Ready(NetworkBehaviourAction::ReportObservedAddr { address }) => {
                    self.topology.add_local_external_addrs(self.raw_swarm.nat_traversal(&address));
                },
            }
        }
    }
}

/// A behaviour for the network. Allows customizing the swarm.
///
/// This trait has been designed to be composable. Multiple implementations can be combined into
/// one that handles all the behaviours at once.
pub trait NetworkBehaviour<TTopology> {
    /// Handler for all the protocols the network supports.
    type ProtocolsHandler: ProtocolsHandler;
    /// Event generated by the swarm.
    type OutEvent;

    /// Builds a new `ProtocolsHandler`.
    fn new_handler(&mut self) -> Self::ProtocolsHandler;

    /// Indicates the behaviour that we connected to the node with the given peer id through the
    /// given endpoint.
    fn inject_connected(&mut self, peer_id: PeerId, endpoint: ConnectedPoint);

    /// Indicates the behaviour that we disconnected from the node with the given peer id. The
    /// endpoint is the one we used to be connected to.
    fn inject_disconnected(&mut self, peer_id: &PeerId, endpoint: ConnectedPoint);

    /// Indicates the behaviour that the node with the given peer id has generated an event for
    /// us.
    ///
    /// > **Note**: This method is only called for events generated by the protocols handler.
    fn inject_node_event(
        &mut self,
        peer_id: PeerId,
        event: <Self::ProtocolsHandler as ProtocolsHandler>::OutEvent
    );

    /// Polls for things that swarm should do.
    ///
    /// This API mimics the API of the `Stream` trait.
    fn poll(&mut self, topology: &mut PollParameters<TTopology>) -> Async<NetworkBehaviourAction<<Self::ProtocolsHandler as ProtocolsHandler>::InEvent, Self::OutEvent>>;
}

/// Used when deriving `NetworkBehaviour`. When deriving `NetworkBehaviour`, must be implemented
/// for all the possible event types generated by the various fields.
// TODO: document how the custom behaviour works and link this here
pub trait NetworkBehaviourEventProcess<TEvent> {
    /// Called when one of the fields of the type you're deriving `NetworkBehaviour` on generates
    /// an event.
    fn inject_event(&mut self, event: TEvent);
}

/// Parameters passed to `poll()`, that the `NetworkBehaviour` has access to.
// TODO: #[derive(Debug)]
pub struct PollParameters<'a, TTopology: 'a> {
    topology: &'a mut TTopology,
    supported_protocols: &'a [Vec<u8>],
    listened_addrs: &'a [Multiaddr],
    nat_traversal: &'a dyn Fn(&Multiaddr, &Multiaddr) -> Option<Multiaddr>,
}

impl<'a, TTopology> PollParameters<'a, TTopology> {
    /// Returns a reference to the topology of the network.
    #[inline]
    pub fn topology(&mut self) -> &mut TTopology {
        &mut self.topology
    }

    /// Returns the list of protocol the behaviour supports when a remote negotiates a protocol on
    /// an inbound substream.
    ///
    /// The iterator's elements are the ASCII names as reported on the wire.
    ///
    /// Note that the list is computed once at initialization and never refreshed.
    #[inline]
    pub fn supported_protocols(&self) -> impl ExactSizeIterator<Item = &[u8]> {
        self.supported_protocols.iter().map(AsRef::as_ref)
    }

    /// Returns the list of the addresses we're listening on.
    #[inline]
    pub fn listened_addresses(&self) -> impl ExactSizeIterator<Item = &Multiaddr> {
        self.listened_addrs.iter()
    }

    /// Returns the list of the addresses nodes can use to reach us.
    #[inline]
    pub fn external_addresses<'b>(&'b mut self) -> impl ExactSizeIterator<Item = Multiaddr> + 'b
    where TTopology: Topology
    {
        let local_peer_id = self.topology.local_peer_id().clone();
        self.topology.addresses_of_peer(&local_peer_id).into_iter()
    }

    /// Returns the public key of the local node.
    #[inline]
    pub fn local_public_key(&self) -> &PublicKey
    where TTopology: Topology
    {
        self.topology.local_public_key()
    }

    /// Returns the peer id of the local node.
    #[inline]
    pub fn local_peer_id(&self) -> &PeerId
    where TTopology: Topology
    {
        self.topology.local_peer_id()
    }

    /// Calls the `nat_traversal` method on the underlying transport of the `Swarm`.
    #[inline]
    pub fn nat_traversal(&self, server: &Multiaddr, observed: &Multiaddr) -> Option<Multiaddr> {
        (self.nat_traversal)(server, observed)
    }
}

/// Action to perform.
#[derive(Debug, Clone)]
pub enum NetworkBehaviourAction<TInEvent, TOutEvent> {
    /// Generate an event for the outside.
    GenerateEvent(TOutEvent),

    // TODO: report new raw connection for usage after intercepting an address dial

    /// Instructs the swarm to dial the given multiaddress without any expectation of a peer id.
    DialAddress {
        /// The address to dial.
        address: Multiaddr,
    },

    /// Instructs the swarm to try reach the given peer.
    DialPeer {
        /// The peer to try reach.
        peer_id: PeerId,
    },

    /// If we're connected to the given peer, sends a message to the protocol handler.
    ///
    /// If we're not connected to this peer, does nothing. If necessary, the implementation of
    /// `NetworkBehaviour` is supposed to track which peers we are connected to.
    SendEvent {
        /// The peer which to send the message to.
        peer_id: PeerId,
        /// Event to send to the peer.
        event: TInEvent,
    },

    /// Reports that a remote observes us as this address.
    ///
    /// The swarm will pass this address through the transport's NAT traversal.
    ReportObservedAddr {
        /// The address we're being observed as.
        address: Multiaddr,
    },
}
