package proxy

import (
	"log/slog"
	"net"
	"testing"

	"github.com/sandertv/gophertunnel/minecraft"
)

// A handler's dialer carries the target and every pre-login transfer hop, so a server cannot
// transfer a session past the caller's policy.
func TestSessionHandlerJoinsAndHopsThroughItsDialer(t *testing.T) {
	dialer := new(net.Dialer)
	handler, err := NewSessionHandler(SessionHandlerConfig{Dialer: dialer, Logger: slog.New(slog.DiscardHandler)})
	if err != nil {
		t.Fatal(err)
	}
	defer handler.Close()
	target, err := handler.server.dialTarget(t.Context(), "entry.example:19132")
	if err != nil {
		t.Fatal(err)
	}
	for _, address := range []string{"entry.example:19132", "hop.example:19132"} {
		network, ok := handler.server.prepared.hopNetwork(target, address).(minecraft.RakNet)
		if !ok || network.UpstreamDialer != dialer {
			t.Fatalf("%s joins through %#v, want RakNet over the handler's dialer", address, network)
		}
	}
	if _, err := NewSessionHandler(SessionHandlerConfig{}); err == nil {
		t.Fatal("a handler without a dialer was created")
	}
}
