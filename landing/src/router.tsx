import { createRootRoute, createRoute, Outlet } from '@tanstack/react-router'
import { LandingPage } from './routes/index'

const rootRoute = createRootRoute({
  component: () => <Outlet />,
})

const indexRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/',
  component: LandingPage,
})

export const routeTree = rootRoute.addChildren([indexRoute])
