package server

// Server serves.
type Server struct {
	addr string
}

type Handler interface {
	Serve() error
}

type ID string

const DefaultAddr = ":8080"

func New(addr string) *Server {
	return &Server{addr: addr}
}

func (s *Server) Start() error {
	return nil
}
