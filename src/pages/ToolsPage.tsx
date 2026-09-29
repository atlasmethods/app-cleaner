import { Tile } from '../components/Tile';
import { TOOLS } from '../nav';

export default function ToolsPage() {
  return (
    <div data-testid="page-tools" className="p-4">
      <div className="grid grid-cols-3 gap-2">
        {TOOLS.map((t) => (
          <Tile key={t.id} to={t.path} label={t.label} icon={t.icon} testId={`tile-${t.id}`} />
        ))}
      </div>
    </div>
  );
}
