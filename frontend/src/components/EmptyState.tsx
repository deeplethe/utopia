/** Consistent empty / all-clear / no-match state across the Review tabs — a plain muted line
 *  (no box, no accent icon) so every list settles into the same calm state. */
export default function EmptyState({ message }: { message: string }) {
  return <p className="py-10 text-center text-sm text-muted-foreground">{message}</p>
}
