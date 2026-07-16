import {
  arrow,
  autoUpdate,
  flip,
  FloatingArrow,
  FloatingPortal,
  offset,
  shift,
  useFloating,
  useFocus,
  useHover,
  useInteractions,
  type Placement,
  type VirtualElement,
} from "@floating-ui/react";
import {
  cloneElement,
  isValidElement,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type FocusEvent,
  type JSX,
  type MutableRefObject,
  type PointerEvent,
  type ReactElement,
  type ReactNode,
  type Ref,
} from "react";
import "./popover.css";

export type PopoverTrigger = "hover" | "focus";

export interface PopoverProps {
  title?: ReactNode;
  content: ReactNode;
  children?: ReactElement;
  trigger?: PopoverTrigger | PopoverTrigger[];
  placement?: Placement;
  open?: boolean;
  defaultOpen?: boolean;
  onOpenChange?: (open: boolean) => void;
  disabled?: boolean;
  arrow?: boolean;
  overlayClassName?: string;
  getPopupContainer?: (triggerNode: HTMLElement | null) => HTMLElement;
  reference?: Element | VirtualElement | null;
}

type MaybeHandler<E> = ((event: E) => void) | undefined;
type PopoverChildProps = {
  onPointerEnter?: MaybeHandler<PointerEvent<HTMLElement>>;
  onPointerLeave?: MaybeHandler<PointerEvent<HTMLElement>>;
  onFocus?: MaybeHandler<FocusEvent<HTMLElement>>;
  onBlur?: MaybeHandler<FocusEvent<HTMLElement>>;
  ref?: Ref<HTMLElement>;
};

type ReferenceEffectName = "effect" | "layout";

export function popoverReferenceEffectName(scope: Pick<typeof globalThis, "document"> | Record<string, unknown> = globalThis): ReferenceEffectName {
  return "document" in scope ? "layout" : "effect";
}

const usePopoverReferenceEffect =
  popoverReferenceEffectName() === "layout" ? useLayoutEffect : useEffect;

export function composeEventHandlers<E>(
  userHandler: MaybeHandler<E>,
  injectedHandler: MaybeHandler<E>,
): (event: E) => void {
  return (event) => {
    userHandler?.(event);
    injectedHandler?.(event);
  };
}

export function isRenderablePopoverContent(content: ReactNode): boolean {
  if (content === null || content === undefined || content === false) return false;
  if (typeof content === "string" && content.length === 0) return false;
  return true;
}

function normalizeTriggers(trigger: PopoverTrigger | PopoverTrigger[]): PopoverTrigger[] {
  return Array.isArray(trigger) ? trigger : [trigger];
}

function mergeClassName(base: string, extra: string | undefined): string {
  return extra ? `${base} ${extra}` : base;
}

function setRef(ref: Ref<HTMLElement> | undefined, value: HTMLElement | null): void {
  if (typeof ref === "function") ref(value);
  else if (ref && typeof ref === "object") {
    (ref as MutableRefObject<HTMLElement | null>).current = value;
  }
}

export function Popover({
  title,
  content,
  children,
  trigger = "hover",
  placement = "top",
  open,
  defaultOpen = false,
  onOpenChange,
  disabled = false,
  arrow: showArrow = true,
  overlayClassName,
  getPopupContainer,
  reference,
}: PopoverProps): JSX.Element {
  const arrowRef = useRef<SVGSVGElement | null>(null);
  const triggerRef = useRef<HTMLElement | null>(null);
  const [uncontrolledOpen, setUncontrolledOpen] = useState(defaultOpen);
  const controlled = open !== undefined;
  const actualOpen = controlled ? open : uncontrolledOpen;
  const hasReferenceProp = reference !== undefined;
  const hasContent = isRenderablePopoverContent(content);
  const triggers = useMemo(() => normalizeTriggers(trigger), [trigger]);

  const handleOpenChange = (nextOpen: boolean) => {
    if (disabled) return;
    if (!controlled) setUncontrolledOpen(nextOpen);
    onOpenChange?.(nextOpen);
  };

  const { refs, floatingStyles, context } = useFloating({
    open: Boolean(actualOpen && hasContent && !disabled),
    onOpenChange: handleOpenChange,
    placement,
    whileElementsMounted: autoUpdate,
    middleware: [
      offset(7),
      flip({ padding: 8 }),
      shift({ padding: 8 }),
      ...(showArrow ? [arrow({ element: arrowRef })] : []),
    ],
  });

  usePopoverReferenceEffect(() => {
    if (hasReferenceProp) refs.setReference(reference ?? null);
  }, [hasReferenceProp, reference, refs]);

  const hover = useHover(context, {
    enabled: !disabled && !hasReferenceProp && triggers.includes("hover"),
  });
  const focus = useFocus(context, {
    enabled: !disabled && !hasReferenceProp && triggers.includes("focus"),
  });
  const { getReferenceProps, getFloatingProps } = useInteractions([hover, focus]);

  const referenceProps = getReferenceProps() as PopoverChildProps;
  const child =
    children && isValidElement<PopoverChildProps>(children)
      ? cloneElement(children, {
          ...referenceProps,
          ref: (node: HTMLElement | null) => {
            triggerRef.current = node;
            refs.setReference(node);
            setRef(children.props.ref, node);
          },
          onPointerEnter: composeEventHandlers(children.props.onPointerEnter, referenceProps.onPointerEnter),
          onPointerLeave: composeEventHandlers(children.props.onPointerLeave, referenceProps.onPointerLeave),
          onFocus: composeEventHandlers(children.props.onFocus, referenceProps.onFocus),
          onBlur: composeEventHandlers(children.props.onBlur, referenceProps.onBlur),
        })
      : children;

  const shouldRenderOverlay = Boolean(hasContent && !disabled && actualOpen && (!hasReferenceProp || reference));
  const portalRoot = getPopupContainer?.(triggerRef.current) ?? undefined;
  const overlay = shouldRenderOverlay ? (
    <div
      {...getFloatingProps({
        ref: refs.setFloating,
        className: mergeClassName("tf-popover", overlayClassName),
        style: floatingStyles,
        role: "tooltip",
      })}
    >
      <div className="tf-popover__body">
        {isRenderablePopoverContent(title) ? <div className="tf-popover__title">{title}</div> : null}
        <div className="tf-popover__content">{content}</div>
      </div>
      {showArrow ? <FloatingArrow ref={arrowRef} context={context} className="tf-popover__arrow" /> : null}
    </div>
  ) : null;
  const canUseDom = typeof document !== "undefined";

  return (
    <>
      {child}
      {shouldRenderOverlay ? canUseDom ? <FloatingPortal root={portalRoot}>{overlay}</FloatingPortal> : overlay : null}
    </>
  );
}
