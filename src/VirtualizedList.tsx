import { Fragment, useEffect, useRef, useState, type ReactNode } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import { tableWindow } from './pagination';

export const VIRTUALIZATION_THRESHOLD = 200;

function usePrinting() {
  const [printing, setPrinting] = useState(false);
  useEffect(() => {
    const begin = () => setPrinting(true);
    const end = () => setPrinting(false);
    window.addEventListener('beforeprint', begin);
    window.addEventListener('afterprint', end);
    return () => {
      window.removeEventListener('beforeprint', begin);
      window.removeEventListener('afterprint', end);
    };
  }, []);
  return printing;
}

export function VirtualizedList<T>({
  items,
  className,
  estimateSize,
  getKey,
  renderItem,
  ariaLabel,
}: {
  items: readonly T[];
  className: string;
  estimateSize: number;
  getKey: (item: T) => string;
  renderItem: (item: T) => ReactNode;
  ariaLabel: string;
}) {
  const parentRef = useRef<HTMLDivElement>(null);
  const printing = usePrinting();
  const virtualized = items.length > VIRTUALIZATION_THRESHOLD && !printing;
  const virtualizer = useVirtualizer({
    count: virtualized ? items.length : 0,
    getScrollElement: () => parentRef.current,
    estimateSize: () => estimateSize,
    getItemKey: (index) => getKey(items[index]),
    overscan: 8,
    useFlushSync: false,
  });

  if (!virtualized) return <div className={className}>{items.map(renderItem)}</div>;

  return (
    <div ref={parentRef} className={`${className} virtualized-list`} tabIndex={0} role="region" aria-label={`${ariaLabel} — ${items.length} سجل محمل`}>
      <div className="virtualized-list__canvas" style={{ height: `${virtualizer.getTotalSize()}px` }}>
        {virtualizer.getVirtualItems().map((virtualRow) => (
          <div
            className="virtualized-list__item"
            data-index={virtualRow.index}
            key={virtualRow.key}
            ref={virtualizer.measureElement}
            style={{ transform: `translateY(${virtualRow.start}px)` }}
          >
            {renderItem(items[virtualRow.index])}
          </div>
        ))}
      </div>
    </div>
  );
}

export function WindowedTableBody<T>({
  items,
  renderRow,
  getKey,
  columnCount,
  label,
}: {
  items: readonly T[];
  renderRow: (item: T) => ReactNode;
  getKey: (item: T) => string;
  columnCount: number;
  label: string;
}) {
  const printing = usePrinting();
  const [page, setPage] = useState(0);
  const windowSize = VIRTUALIZATION_THRESHOLD;
  const { page: boundedPage, pages, start, end } = tableWindow(items.length, page, windowSize, printing);

  useEffect(() => {
    setPage((current) => Math.min(current, Math.max(0, pages - 1)));
  }, [pages]);

  return (
    <>
      <tbody>{items.slice(start, end).map((item) => <Fragment key={getKey(item)}>{renderRow(item)}</Fragment>)}</tbody>
      {!printing && pages > 1 && (
        <tfoot className="windowed-table-nav">
          <tr><td colSpan={columnCount}>
            <div className="windowed-table-nav__content">
              <span>{label}: {start + 1}–{end} من {items.length}</span>
              <div>
                <button type="button" disabled={boundedPage === 0} onClick={() => setPage((current) => Math.max(0, current - 1))}>الأحدث</button>
                <button type="button" disabled={boundedPage >= pages - 1} onClick={() => setPage((current) => Math.min(pages - 1, current + 1))}>الأقدم</button>
              </div>
            </div>
          </td></tr>
        </tfoot>
      )}
    </>
  );
}
