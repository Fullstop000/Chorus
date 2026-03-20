interface Props {
  onClose: () => void
  onCreated: () => void
}

export function CreateAgentModal({ onClose }: Props) {
  return (
    <div style={{ position: 'fixed', inset: 0, background: 'rgba(0,0,0,0.4)', display: 'flex', alignItems: 'center', justifyContent: 'center', zIndex: 100 }}>
      <div style={{ background: '#fff', padding: 24, borderRadius: 8 }}>
        <p>Create Agent (stub)</p>
        <button onClick={onClose}>Close</button>
      </div>
    </div>
  )
}
